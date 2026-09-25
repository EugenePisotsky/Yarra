//! Bounded A/B recordings and launch/reset snapshots, independent of F1 UI.
use super::{
    telemetry::{PendingPipelines, Telemetry, scene_size},
    timing,
};
use crate::{
    frame_pacing::{FramePacing, FrameRate},
    game_render::RenderPath,
    runtime_settings::RuntimeSettings,
};
use bevy::{prelude::*, window::PrimaryWindow};
use engine::WorldViewCamera;

const SAMPLE_SECONDS: f64 = 10.0;
const SETTLE_SECONDS: f64 = 2.0;
const MAX_SETTLE_SECONDS: f64 = 8.0;
#[derive(Resource)]
pub(crate) struct CaptureSession {
    baseline: RuntimeSettings,
    baseline_rate: FrameRate,
    pub(super) slots: [Option<Capture>; 2],
    active: Option<Recording>,
    pub(crate) message: String,
}
impl Default for CaptureSession {
    fn default() -> Self {
        Self {
            baseline: default(),
            baseline_rate: default(),
            slots: [None, None],
            active: None,
            message: "Settings are temporary. Reset restores this launch's configuration.".into(),
        }
    }
}
pub(super) struct Progress {
    pub slot: usize,
    pub sampling: bool,
    pub elapsed: f64,
}
impl CaptureSession {
    pub(crate) fn recording(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn remember_launch(&mut self, settings: &RuntimeSettings, rate: FrameRate) {
        self.baseline = settings.clone();
        self.baseline_rate = rate;
    }

    pub(crate) fn reset(&self, settings: &mut RuntimeSettings, pacing: &mut FramePacing) {
        if !self.recording() {
            *settings = self.baseline.clone();
            pacing.rate = self.baseline_rate;
        }
    }

    pub(super) fn progress(&self, now: f64) -> Option<Progress> {
        self.active.as_ref().map(|r| Progress {
            slot: r.slot,
            sampling: r.sampling_since.is_some(),
            elapsed: now - r.sampling_since.unwrap_or(r.started),
        })
    }

    pub(super) fn start(
        &mut self,
        slot: usize,
        settings: &mut RuntimeSettings,
        rate: FrameRate,
        now: f64,
    ) -> bool {
        if self.recording() || slot >= self.slots.len() {
            return false;
        }
        let was_detailed = settings.gpu_pass_timings;
        settings.gpu_pass_timings = false;
        self.active = Some(Recording {
            slot,
            started: now,
            sampling_since: None,
            timing_start: default(),
            was_locked: settings.controls_locked,
            was_detailed,
            capture: Capture::new(settings.clone(), rate),
        });
        settings.controls_locked = true;
        true
    }

    pub(super) fn cancel(&mut self, settings: &mut RuntimeSettings, message: &str) -> bool {
        let Some(recording) = self.active.take() else {
            return false;
        };
        recording.restore_controls(settings);
        self.message = message.into();
        true
    }

    pub(super) fn restore(
        &mut self,
        slot: usize,
        settings: &mut RuntimeSettings,
        pacing: &mut FramePacing,
        now: f64,
    ) {
        if self.recording() {
            return;
        }
        if let Some(Some(capture)) = self.slots.get(slot) {
            *settings = capture.settings.clone();
            pacing.rate = capture.frame_rate;
            settings.changed_at = now;
            self.message = format!(
                "Restored {} settings; viewpoint unchanged.",
                ['A', 'B'][slot]
            );
        }
    }
}

pub(super) fn initialize(
    settings: Res<RuntimeSettings>,
    pacing: Res<FramePacing>,
    mut session: ResMut<CaptureSession>,
) {
    session.remember_launch(&settings, pacing.rate);
}

struct Recording {
    slot: usize,
    started: f64,
    sampling_since: Option<f64>,
    timing_start: timing::Stamp,
    was_locked: bool,
    was_detailed: bool,
    capture: Capture,
}
impl Recording {
    fn restore_controls(&self, settings: &mut RuntimeSettings) {
        settings.controls_locked = self.was_locked;
        settings.gpu_pass_timings = self.was_detailed;
    }
}
#[derive(Clone, Debug)]
pub(super) struct Capture {
    pub(super) settings: RuntimeSettings,
    pub(super) frame_rate: FrameRate,
    pub(super) frames: Vec<f64>,
    pub(super) app_times: Vec<f64>,
    pub(super) thermal_start: String,
    pub(super) thermal_end: String,
    pub(super) context: String,
    pub(super) upscaler: Option<upscaling::UpscaleStatus>,
    pub(super) camera: Mat4,
    pub(super) phase: f32,
    pub(super) weather: Option<engine::WeatherParams>,
    pub(super) viewport: UVec2,
    pub(super) warnings: Vec<String>,
    pub(super) gpu: Vec<timing::GpuSample>,
    pub(super) cpu: Vec<timing::CpuSample>,
}
impl Capture {
    pub(super) fn new(settings: RuntimeSettings, frame_rate: FrameRate) -> Self {
        Self {
            settings,
            frame_rate,
            frames: vec![],
            app_times: vec![],
            thermal_start: String::new(),
            thermal_end: String::new(),
            context: String::new(),
            upscaler: None,
            camera: Mat4::IDENTITY,
            phase: 0.0,
            weather: None,
            viewport: UVec2::ZERO,
            warnings: vec![],
            gpu: vec![],
            cpu: vec![],
        }
    }
}

/// Returns whether a recording ended; presentation is the caller's responsibility.
#[allow(clippy::too_many_arguments)]
pub(super) fn sample(
    time: Res<Time<Real>>,
    window: Single<&Window, With<PrimaryWindow>>,
    t: Res<Telemetry>,
    mut state: ResMut<CaptureSession>,
    mut settings: ResMut<RuntimeSettings>,
    pending: Res<PendingPipelines>,
    streaming: Res<engine::StreamingStats>,
    terrain: Res<engine::TerrainLodStats>,
    camera: Single<(&GlobalTransform, &Camera), With<WorldViewCamera>>,
    atmosphere: Res<engine::AtmosphereState>,
    upscaler: Query<&upscaling::UpscaleStatus, With<WorldViewCamera>>,
    timings: Res<timing::History>,
    stamp: Res<timing::Stamp>,
    pacing: Res<FramePacing>,
) -> bool {
    let frame_ms = time.delta_secs_f64() * 1000.0;
    let app_ms = t.app_ms;
    let Some(recording) = state.active.as_mut() else {
        return false;
    };
    if pacing.rate != recording.capture.frame_rate {
        return state.cancel(&mut settings, "Capture cancelled: FPS limit changed.");
    }
    let now = time.elapsed_secs_f64();
    let busy = pending.count() > 0
        || streaming.loading > 0
        || terrain.quality_pending
        || upscaler.single().is_ok_and(|s| {
            settings.render_path == RenderPath::Composite
                && (s.active.is_none() || s.requested != settings.upscaler)
        });
    let mut cancel = None;
    let mut done = false;
    if !window.focused {
        cancel = Some("Capture cancelled: window lost focus.");
    } else if let Some(start) = recording.sampling_since {
        recording.capture.frames.push(frame_ms);
        recording.capture.app_times.push(app_ms);
        if busy {
            add_warning(
                &mut recording.capture,
                "Loading, pipeline compilation or upscaler preparation occurred during this capture.",
            );
        }
        if upscaler.single().ok() != recording.capture.upscaler.as_ref() {
            add_warning(
                &mut recording.capture,
                "Active upscaler or its dimensions changed during capture.",
            );
        }
        if !camera
            .0
            .to_matrix()
            .abs_diff_eq(recording.capture.camera, 0.02)
        {
            add_warning(&mut recording.capture, "Camera moved during capture.");
        }
        if atmosphere.weather != recording.capture.weather {
            add_warning(&mut recording.capture, "Weather changed during capture.");
        }
        done = now - start >= SAMPLE_SECONDS;
    } else if now - recording.started >= SETTLE_SECONDS && !busy {
        recording.sampling_since = Some(now);
        recording.timing_start = *stamp;
        recording.capture.thermal_start =
            crate::render_audit::logging::power_state().thermal.into();
        recording.capture.camera = camera.0.to_matrix();
        recording.capture.phase = atmosphere.phase;
        recording.capture.weather = atmosphere.weather;
        recording.capture.viewport = scene_size(camera.1, upscaler.single().ok());
        recording.capture.context = format!(
            "{}\nTiming instrumentation: {}\n3D viewport: {:?}; window: {:?}; camera: {:?}; phase: {:.5}; weather: {:?}\nTerrain triangles: {}; patches: {}; resident pages: {}",
            streaming.status,
            timings.status,
            recording.capture.viewport,
            window.physical_size(),
            camera.0.to_matrix(),
            atmosphere.phase,
            atmosphere.weather,
            terrain.triangles,
            terrain.patches,
            streaming.resident
        );
        if let Ok(status) = upscaler.single() {
            recording.capture.context += &format!("\n{}", status.description());
            recording.capture.upscaler = Some(status.clone());
        }
    } else if now - recording.started >= MAX_SETTLE_SECONDS {
        cancel = Some(
            "Capture cancelled: render resources did not settle within 8 s. Try again once the scene is ready.",
        );
    }
    if let Some(message) = cancel {
        return state.cancel(&mut settings, message);
    } else if done {
        let mut recording = state.active.take().unwrap();
        recording.capture.gpu = timings
            .gpu
            .iter()
            .filter(|s| !s.stamp.detailed && in_capture(s.stamp, recording.timing_start, *stamp))
            .cloned()
            .collect();
        recording.capture.cpu = timings
            .cpu
            .iter()
            .filter(|s| in_capture(s.stamp, recording.timing_start, *stamp))
            .cloned()
            .collect();
        if !timings.enabled {
            add_warning(
                &mut recording.capture,
                "Timing probes were disabled at launch; this capture contains frame/app timings only.",
            );
        } else if recording.capture.gpu.len() < 5 {
            add_warning(
                &mut recording.capture,
                "Insufficient GPU timestamp samples; no GPU estimate available.",
            );
        }
        if stamp.epoch != recording.timing_start.epoch {
            add_warning(
                &mut recording.capture,
                "Settings changed during capture; timing samples excluded after the change.",
            );
        }
        recording.capture.thermal_end = crate::render_audit::logging::power_state().thermal.into();
        if recording.capture.thermal_start != recording.capture.thermal_end {
            add_warning(
                &mut recording.capture,
                "Thermal state changed during capture.",
            );
        }
        recording.restore_controls(&mut settings);
        state.message = format!(
            "Captured {}. {}",
            ['A', 'B'][recording.slot],
            if timings.enabled {
                "Compare GPU and CPU work; FPS may stay capped."
            } else {
                "Frame/app timings only; CPU/GPU probes are disabled."
            }
        );
        state.slots[recording.slot] = Some(recording.capture);
        return true;
    }
    false
}
fn add_warning(capture: &mut Capture, warning: &str) {
    if !capture.warnings.iter().any(|w| w == warning) {
        capture.warnings.push(warning.into());
    }
}
fn in_capture(sample: timing::Stamp, start: timing::Stamp, end: timing::Stamp) -> bool {
    sample.epoch == start.epoch && sample.frame > start.frame && sample.frame <= end.frame
}

#[cfg(test)]
mod tests;
