//! Rolling frame/app telemetry and cross-world pipeline readiness.
use crate::frame_pacing::FramePacing;
use bevy::{
    prelude::*,
    render::{Render, RenderApp, RenderSystems, render_resource::PipelineCache},
    window::PrimaryWindow,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

#[derive(Resource, Default)]
pub(super) struct Telemetry {
    pub(super) frames: VecDeque<f64>,
    pub(super) graph: VecDeque<f64>,
    pub(super) peak_ms: f64,
    pub(super) app_times: VecDeque<f64>,
    start: Option<Instant>,
    pub(super) app_ms: f64,
    pub(super) refreshed: f64,
}
#[derive(Resource, Clone, Default)]
pub(super) struct PendingPipelines(Arc<AtomicUsize>);

impl PendingPipelines {
    pub(super) fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}
pub(super) fn install(app: &mut App) {
    let pending = PendingPipelines::default();
    app.insert_resource(pending.clone())
        .init_resource::<Telemetry>()
        .add_systems(First, begin_frame)
        .add_systems(Last, end_frame);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .insert_resource(pending)
            .add_systems(Render, track_pipelines.in_set(RenderSystems::Cleanup));
    }
}
fn track_pipelines(cache: Res<PipelineCache>, pending: Res<PendingPipelines>) {
    pending
        .0
        .store(cache.waiting_pipelines().count(), Ordering::Relaxed);
}
fn begin_frame(mut t: ResMut<Telemetry>) {
    t.start = Some(Instant::now());
}
fn end_frame(mut t: ResMut<Telemetry>) {
    if let Some(start) = t.start {
        t.app_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
}

fn push_history(values: &mut VecDeque<f64>, value: f64) {
    if values.len() == 240 {
        values.pop_front();
    }
    values.push_back(value);
}
pub(super) fn sample(
    time: Res<Time<Real>>,
    window: Single<&Window, With<PrimaryWindow>>,
    pacing: Res<FramePacing>,
    mut t: ResMut<Telemetry>,
) {
    if pacing.is_changed() {
        // Averages from the previous cap must not masquerade as the new FPS.
        t.frames.clear();
        t.app_times.clear();
    }
    let frame_ms = time.delta_secs_f64() * 1000.0;
    let app_ms = t.app_ms;
    if window.focused {
        t.peak_ms = t.peak_ms.max(frame_ms);
    }
    if window.focused && frame_ms > 0.0 {
        push_history(&mut t.frames, frame_ms);
        push_history(&mut t.app_times, app_ms);
    }
}
pub(super) fn scene_size(camera: &Camera, status: Option<&upscaling::UpscaleStatus>) -> UVec2 {
    status.filter(|s| s.input_size != UVec2::ZERO).map_or_else(
        || camera.physical_viewport_size().unwrap_or_default(),
        |s| s.input_size,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn histories_are_bounded_ignore_unfocused_frames_and_restart_after_fps_changes() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<FramePacing>()
            .init_resource::<Telemetry>()
            .add_systems(Update, sample);
        let window = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        for _ in 0..250 {
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(std::time::Duration::from_millis(10));
            app.update();
        }
        assert_eq!(app.world().resource::<Telemetry>().frames.len(), 240);
        assert_eq!(app.world().resource::<Telemetry>().app_times.len(), 240);
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_millis(100));
        app.update();
        assert_eq!(
            app.world().resource::<Telemetry>().frames.back(),
            Some(&10.0)
        );
        assert_eq!(app.world().resource::<Telemetry>().peak_ms, 10.0);
        app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
        app.world_mut().resource_mut::<FramePacing>().rate =
            crate::frame_pacing::FrameRate::new(30);
        app.update();
        let telemetry = app.world().resource::<Telemetry>();
        assert_eq!(telemetry.frames.len(), 1);
        assert_eq!(telemetry.app_times.len(), 1);
        assert_eq!(telemetry.frames.back(), Some(&100.0));
    }
}
