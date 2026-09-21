//! In-game diagnostics. Temporary settings and bounded, manually requested comparisons.
use super::timing::{self, Kind};
use super::*;
use bevy::render::{Render, RenderApp, RenderSystems, render_resource::PipelineCache};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const SAMPLE_SECONDS: f64 = 10.0;
const SETTLE_SECONDS: f64 = 2.0;
const MAX_SETTLE_SECONDS: f64 = 8.0;
const ALL_CONTROLS: &[Control] = &[
    Control::FrameRate,
    Control::Clouds,
    Control::Sky,
    Control::Bloom,
    Control::Grass,
    Control::Terrain,
    Control::Objects,
    Control::Shadows,
    Control::Wind,
    Control::Scale,
    Control::Upscaler,
    Control::TemporalDebug,
    Control::Antialiasing,
    Control::Density,
    Control::TerrainDetail,
    Control::ObjectDetail,
    Control::Near,
    Control::GroundMaterial,
    Control::Prepass,
    Control::Shading,
    Control::Counters,
    Control::GpuPassTimings,
    Control::RenderPath,
    Control::Scene,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Features,
    Quality,
    Compare,
    Advanced,
}
impl Tab {
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Features => "Features",
            Self::Quality => "Quality",
            Self::Compare => "Compare",
            Self::Advanced => "Advanced",
        }
    }
}
#[derive(Component)]
struct PanelRoot;
#[derive(Component)]
struct Page(Tab);
#[derive(Component)]
struct Summary;
#[derive(Component)]
struct Details;
#[derive(Component)]
struct Comparison;
#[derive(Component)]
struct PassTimes;
#[derive(Component)]
struct Notice;
#[derive(Component)]
struct Bar(usize);
#[derive(Component, Clone, Copy)]
enum Action {
    Toggle,
    Tab(Tab),
    Capture(usize),
    Restore(usize),
    Export,
}

#[derive(Resource)]
pub(super) struct PanelState {
    open: bool,
    tab: Tab,
    pub(super) baseline: AuditSettings,
    pub(super) baseline_rate: FrameRate,
    slots: [Option<Capture>; 2],
    active: Option<Recording>,
    message: String,
}
impl Default for PanelState {
    fn default() -> Self {
        Self {
            open: std::env::args_os().any(|a| a == "--performance-open" || a == "--render-audit"),
            tab: Tab::Overview,
            baseline: default(),
            baseline_rate: default(),
            slots: [None, None],
            active: None,
            message: "Settings are temporary. Reset restores this launch's configuration.".into(),
        }
    }
}
impl PanelState {
    pub(super) fn recording(&self) -> bool {
        self.active.is_some()
    }
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
    fn restore_controls(&self, settings: &mut AuditSettings) {
        settings.controls_locked = self.was_locked;
        settings.gpu_pass_timings = self.was_detailed;
    }
}
#[derive(Clone, Debug)]
struct Capture {
    settings: AuditSettings,
    frame_rate: FrameRate,
    frames: Vec<f64>,
    app_times: Vec<f64>,
    thermal_start: String,
    thermal_end: String,
    context: String,
    upscaler: Option<upscaling::UpscaleStatus>,
    camera: Mat4,
    phase: f32,
    viewport: UVec2,
    warnings: Vec<String>,
    gpu: Vec<timing::GpuSample>,
    cpu: Vec<timing::CpuSample>,
}
#[derive(Resource, Default)]
struct Telemetry {
    frames: VecDeque<f64>,
    graph: VecDeque<f64>,
    peak_ms: f64,
    app_times: VecDeque<f64>,
    start: Option<Instant>,
    app_ms: f64,
    refreshed: f64,
    thermal: String,
}
#[derive(Resource, Clone, Default)]
struct PendingPipelines(Arc<AtomicUsize>);

pub(super) fn install(app: &mut App) {
    let pending = PendingPipelines::default();
    app.insert_resource(pending.clone())
        .init_resource::<PanelState>()
        .init_resource::<Telemetry>()
        .add_systems(First, begin_frame)
        .add_systems(Last, end_frame)
        .add_systems(Update, (actions, sample).chain().before(super::buttons))
        .add_systems(PostUpdate, (refresh, visibility).chain());
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

pub(super) fn initialize(
    mut commands: Commands,
    mut s: ResMut<AuditSettings>,
    mut state: ResMut<PanelState>,
    clouds: Res<engine::CloudQuality>,
    grass: Res<VegetationDebugSettings>,
    pacing: Res<FramePacing>,
) {
    s.clouds = *clouds;
    s.density = grass.density_mode;
    s.lighting = grass.lighting_mode;
    state.baseline = s.clone();
    state.baseline_rate = pacing.rate;
    commands
        .spawn((
            Button,
            Action::Toggle,
            AuditPanel,
            Node {
                position_type: PositionType::Absolute,
                right: px(14),
                top: px(12),
                padding: UiRect::axes(px(14), px(9)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.08, 0.11, 0.92)),
            GlobalZIndex(110),
        ))
        .with_child((Text::new("Performance  |  F1"), font(14.0), Notice));
    commands.spawn((PanelRoot, AuditPanel, Node {
        position_type: PositionType::Absolute, right: px(14), top: px(54), width: px(490),
        max_width: percent(96), max_height: percent(88), padding: UiRect::all(px(16)),
        flex_direction: FlexDirection::Column, row_gap: px(10), overflow: Overflow::scroll_y(), ..default()
    }, ScrollPosition::default(), BackgroundColor(Color::srgba(0.025, 0.04, 0.055, 0.98)), GlobalZIndex(100)))
    .with_children(|panel| {
        panel.spawn((Text::new("PERFORMANCE"), font(18.0)));
        panel.spawn((Text::new("Starting measurements..."), font(14.0), Summary));
        panel.spawn((Button, Control::FrameRate, button_node(), BackgroundColor(button_color())))
            .with_child((Text::new(pacing.control_label()), font(13.0)));
        panel.spawn(Node { column_gap: px(4), flex_wrap: FlexWrap::Wrap, row_gap: px(4), ..default() }).with_children(|row| {
            for tab in [Tab::Overview, Tab::Features, Tab::Quality, Tab::Compare, Tab::Advanced] {
                row.spawn((Button, Action::Tab(tab), button_node(), BackgroundColor(button_color())))
                    .with_child((Text::new(tab.label()), font(12.0)));
            }
        });
        panel.spawn((Page(Tab::Overview), page_node())).with_children(|page| {
            page.spawn((Text::new("Last 15 s | peak frame / 0.25 s | 33 ms full height"), font(12.0)));
            page.spawn(Node { height: px(64), align_items: AlignItems::End, column_gap: px(2), ..default() }).with_children(|graph| {
                for i in 0..60 { graph.spawn((Bar(i), Node { flex_grow: 1.0, height: px(1), ..default() }, BackgroundColor(Color::srgb(0.2, 0.75, 0.65)))); }
            });
            page.spawn((Text::new(""), font(13.0), Details));
            page.spawn((Text::new("GPU elapsed uses asynchronous hardware timestamps around render commands, excluding presentation. CPU work sums measured systems; parallel jobs can overlap. Acquisition and submission may block. These numbers are not added into one frame total."), font(12.0)));
        });
        for (tab, note, controls) in [
            (Tab::Features, "Grass Disabled skips its render preparation, compute and draws; source streaming remains. Terrain/object draw switches keep streaming, animation and collision. Sky + haze keeps authored illumination. Cloud Off skips cloud passes; shared material bookkeeping remains.", &[Control::Clouds, Control::GrassEnabled, Control::Sky, Control::Bloom, Control::Terrain, Control::Objects, Control::Shadows, Control::Wind][..]),
            (Tab::Quality, "Click to cycle. Larger terrain error allows coarser geometry. Object LOD size below 1 selects coarser available assets. Ground contact requirements still apply; grass density changes acceptance and geometry LOD. Lower resolution affects the 3D scene only.", &[Control::Scale, Control::Upscaler, Control::TemporalDebug, Control::Antialiasing, Control::Density, Control::TerrainDetail, Control::ObjectDetail, Control::Near][..]),
            (Tab::Advanced, "Diagnostic experiments. Compute only hides grass draws but keeps generation; Schedule only omits generation. Draw frozen locks controls. These settings do not change published content.", &[Control::Grass, Control::Scene, Control::Shading, Control::GroundMaterial, Control::Prepass, Control::Counters, Control::GpuPassTimings, Control::RenderPath, Control::Lock, Control::Overlays][..]),
        ] {
            panel.spawn((Page(tab), page_node())).with_children(|page| {
                page.spawn((Text::new(note), font(12.0)));
                if tab == Tab::Advanced { page.spawn((Text::new(""), font(12.0), PassTimes)); }
                page.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), ..default() }).with_children(|row| {
                    for &control in controls { row.spawn((Button, control, Node { width: px(218), ..button_node() }, BackgroundColor(button_color()))).with_child((Text::new(control.label(&s, pacing.rate)), font(13.0))); }
                });
            });
        }
        panel.spawn((Page(Tab::Compare), page_node())).with_children(|page| {
            page.spawn((Text::new("Capture current settings into A or B: settle at least 2 s, then record 10 s. Detailed GPU probes pause during capture. View controls lock and this panel closes. F1 / Escape cancels. Slots last for this session. Restore changes settings only; keep the same viewpoint and time of day yourself."), font(12.0)));
            page.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), ..default() }).with_children(|row| {
                for (action,label) in [(Action::Capture(0),"Capture A | 10 s"),(Action::Restore(0),"Restore A settings"),(Action::Capture(1),"Capture B | 10 s"),(Action::Restore(1),"Restore B settings"),(Action::Export,"Export report + frames")] {
                    row.spawn((Button, action, button_node(), BackgroundColor(button_color()))).with_child((Text::new(label), font(13.0)));
                }
            });
            page.spawn((Text::new("A and B have not been captured."), font(13.0), Comparison));
        });
        panel.spawn((Button, Control::Reset, button_node(), BackgroundColor(button_color()))).with_child((Text::new("Reset launch settings"), font(13.0)));
    });
}
fn page_node() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(12),
        ..default()
    }
}
fn button_node() -> Node {
    Node {
        min_height: px(32),
        padding: UiRect::all(px(8)),
        align_items: AlignItems::Center,
        ..default()
    }
}
fn button_color() -> Color {
    Color::srgb(0.10, 0.17, 0.22)
}

fn actions(
    keys: Res<ButtonInput<KeyCode>>,
    clicks: Query<(&Interaction, &Action), Changed<Interaction>>,
    mut state: ResMut<PanelState>,
    mut settings: ResMut<AuditSettings>,
    time: Res<Time<Real>>,
    mut scroll: Query<&mut ScrollPosition, With<PanelRoot>>,
    wheel: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    mut pacing: ResMut<FramePacing>,
) {
    if state.open && wheel.delta.y != 0.0 {
        for mut pos in &mut scroll {
            pos.y = (pos.y - wheel.delta.y * 24.0).max(0.0);
        }
    }
    let mut actions: Vec<_> = clicks
        .iter()
        .filter(|(i, _)| **i == Interaction::Pressed)
        .map(|(_, a)| *a)
        .collect();
    if keys.just_pressed(KeyCode::F1)
        || (keys.just_pressed(KeyCode::Escape) && (state.open || state.recording()))
    {
        actions.push(Action::Toggle);
    }
    for action in actions {
        if matches!(action, Action::Toggle) {
            if let Some(recording) = state.active.take() {
                recording.restore_controls(&mut settings);
                state.message = "Capture cancelled; previous results kept.".into();
                state.open = true;
            } else {
                state.open = !state.open;
            }
            continue;
        }
        if state.recording() {
            continue;
        }
        match action {
            Action::Tab(tab) => {
                state.tab = tab;
                for mut pos in &mut scroll {
                    pos.y = 0.0;
                }
            }
            Action::Restore(slot) => {
                if let Some(capture) = &state.slots[slot] {
                    *settings = capture.settings.clone();
                    pacing.rate = capture.frame_rate;
                    settings.changed_at = time.elapsed_secs_f64();
                    state.message = format!(
                        "Restored {} settings; viewpoint unchanged.",
                        ['A', 'B'][slot]
                    );
                }
            }
            Action::Capture(slot) => {
                let was_detailed = settings.gpu_pass_timings;
                settings.gpu_pass_timings = false;
                let capture = Capture {
                    settings: settings.clone(),
                    frame_rate: pacing.rate,
                    frames: vec![],
                    app_times: vec![],
                    thermal_start: String::new(),
                    thermal_end: String::new(),
                    context: String::new(),
                    upscaler: None,
                    camera: Mat4::IDENTITY,
                    phase: 0.0,
                    viewport: UVec2::ZERO,
                    warnings: vec![],
                    gpu: vec![],
                    cpu: vec![],
                };
                state.active = Some(Recording {
                    slot,
                    started: time.elapsed_secs_f64(),
                    sampling_since: None,
                    timing_start: timing::Stamp::default(),
                    was_locked: settings.controls_locked,
                    was_detailed,
                    capture,
                });
                settings.controls_locked = true;
                state.open = false;
            }
            Action::Export => {
                state.message = match export(&state) {
                    Ok(path) => format!("Exported to {}", path.display()),
                    Err(error) => format!("Export failed: {error}"),
                }
            }
            Action::Toggle => unreachable!(),
        }
    }
}

fn push_history(values: &mut VecDeque<f64>, value: f64) {
    if values.len() == 240 {
        values.pop_front();
    }
    values.push_back(value);
}
#[allow(clippy::too_many_arguments)]
fn sample(
    time: Res<Time<Real>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut t: ResMut<Telemetry>,
    mut state: ResMut<PanelState>,
    mut settings: ResMut<AuditSettings>,
    pending: Res<PendingPipelines>,
    streaming: Res<engine::StreamingStats>,
    terrain: Res<engine::TerrainLodStats>,
    camera: Single<(&GlobalTransform, &Camera), With<WorldViewCamera>>,
    atmosphere: Res<engine::AtmosphereState>,
    upscaler: Query<&upscaling::UpscaleStatus, With<WorldViewCamera>>,
    timings: Res<timing::History>,
    stamp: Res<timing::Stamp>,
    pacing: Res<FramePacing>,
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
    let Some(recording) = state.active.as_mut() else {
        return;
    };
    if pacing.rate != recording.capture.frame_rate {
        let recording = state.active.take().unwrap();
        recording.restore_controls(&mut settings);
        state.open = true;
        state.message = "Capture cancelled: FPS limit changed.".into();
        return;
    }
    let now = time.elapsed_secs_f64();
    let busy = pending.0.load(Ordering::Relaxed) > 0
        || streaming.loading > 0
        || terrain.quality_pending
        || upscaler.single().is_ok_and(|s| {
            settings.render_path == AuditRenderPath::Composite
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
        done = now - start >= SAMPLE_SECONDS;
    } else if now - recording.started >= SETTLE_SECONDS && !busy {
        recording.sampling_since = Some(now);
        recording.timing_start = *stamp;
        recording.capture.thermal_start = super::logging::power_state().thermal.into();
        recording.capture.camera = camera.0.to_matrix();
        recording.capture.phase = atmosphere.phase;
        recording.capture.viewport = scene_size(camera.1, upscaler.single().ok());
        recording.capture.context = format!(
            "{}\n3D viewport: {:?}; window: {:?}; camera: {:?}; phase: {:.5}\nTerrain triangles: {}; patches: {}; resident pages: {}",
            streaming.status,
            recording.capture.viewport,
            window.physical_size(),
            camera.0.to_matrix(),
            atmosphere.phase,
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
        let recording = state.active.take().unwrap();
        recording.restore_controls(&mut settings);
        state.open = true;
        state.message = message.into();
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
        if recording.capture.gpu.len() < 5 {
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
        recording.capture.thermal_end = super::logging::power_state().thermal.into();
        if recording.capture.thermal_start != recording.capture.thermal_end {
            add_warning(
                &mut recording.capture,
                "Thermal state changed during capture.",
            );
        }
        recording.restore_controls(&mut settings);
        state.message = format!(
            "Captured {}. Compare GPU and CPU work; FPS may stay capped.",
            ['A', 'B'][recording.slot]
        );
        state.slots[recording.slot] = Some(recording.capture);
        state.open = true;
    }
}
fn add_warning(capture: &mut Capture, warning: &str) {
    if !capture.warnings.iter().any(|w| w == warning) {
        capture.warnings.push(warning.into());
    }
}
#[derive(Debug, Default)]
struct Stats {
    mean: f64,
    median: f64,
    p95: f64,
    p99: f64,
}
fn stats(values: impl IntoIterator<Item = f64>) -> Stats {
    duration_stats(values.into_iter().filter(|v| *v > 0.0))
}
fn duration_stats(values: impl IntoIterator<Item = f64>) -> Stats {
    let mut v: Vec<_> = values
        .into_iter()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect();
    if v.is_empty() {
        return Stats::default();
    }
    v.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        v[((v.len() as f64 * p).ceil() as usize)
            .saturating_sub(1)
            .min(v.len() - 1)]
    };
    Stats {
        mean: v.iter().sum::<f64>() / v.len() as f64,
        median: percentile(0.5),
        p95: percentile(0.95),
        p99: percentile(0.99),
    }
}
fn comparison(state: &PanelState) -> String {
    let mut out = format!("{}\n", state.message);
    for (i, slot) in state.slots.iter().enumerate() {
        if let Some(c) = slot {
            let s = stats(c.frames.iter().copied());
            out += &format!(
                "\n{} | {} frames | {:.1} FPS average\n{}\nFrame ms: median {:.2} / p95 {:.2} / p99 {:.2}\nThermal: {} -> {}\n",
                ['A', 'B'][i],
                c.frames.len(),
                1000.0 / s.mean,
                c.frame_rate.label(),
                s.median,
                s.p95,
                s.p99,
                c.thermal_start,
                c.thermal_end
            );
            out += &timing_summary(&c.gpu, &c.cpu);
            if let Some(upscaler) = &c.upscaler {
                out += &format!("{}\n", upscaler.description());
            }
            for warning in &c.warnings {
                out += &format!("{warning}\n");
            }
        }
    }
    if let [Some(a), Some(b)] = &state.slots {
        let delta = stats(b.frames.iter().copied()).median - stats(a.frames.iter().copied()).median;
        if a.gpu.len() >= 5 && b.gpu.len() >= 5 {
            out += &format!(
                "\nB - A GPU median: {:+.2} ms\n",
                stats(b.gpu.iter().map(|s| s.elapsed_ms)).median
                    - stats(a.gpu.iter().map(|s| s.elapsed_ms)).median
            );
        }
        out += &format!(
            "\nB - A median: {delta:+.2} ms (paced FPS can conceal headroom)\nSettings changed:\n"
        );
        let mut differences = 0;
        for &control in ALL_CONTROLS {
            let from = control.label(&a.settings, a.frame_rate);
            let to = control.label(&b.settings, b.frame_rate);
            if from != to {
                differences += 1;
                out += &format!("{from} -> {to}\n");
            }
        }
        if differences == 0 {
            out += "None\n";
        }
        if !a.camera.abs_diff_eq(b.camera, 0.02)
            || (a.phase - b.phase).abs() > 0.001
            || a.viewport != b.viewport
            || a.thermal_start != b.thermal_start
            || a.thermal_end != b.thermal_end
        {
            out += "Conditions differ (view / resolution / time / thermal). Do not treat this as an isolated feature comparison.\n";
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn refresh(
    time: Res<Time<Real>>,
    mut t: ResMut<Telemetry>,
    state: Res<PanelState>,
    pending: Res<PendingPipelines>,
    streaming: Res<engine::StreamingStats>,
    terrain: Res<engine::TerrainLodStats>,
    camera: Single<&Camera, With<WorldViewCamera>>,
    upscaler: Query<&upscaling::UpscaleStatus, With<WorldViewCamera>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut texts: ParamSet<(
        Query<&mut Text, (With<Summary>, Without<PassTimes>)>,
        Query<&mut Text, (With<Details>, Without<PassTimes>)>,
        Query<&mut Text, (With<Comparison>, Without<PassTimes>)>,
        Query<&mut Text, (With<Notice>, Without<PassTimes>)>,
    )>,
    timings: Res<timing::History>,
    stamp: Res<timing::Stamp>,
    mut passes: Query<
        &mut Text,
        (
            With<PassTimes>,
            Without<Summary>,
            Without<Details>,
            Without<Comparison>,
            Without<Notice>,
        ),
    >,
    mut bars: Query<(&Bar, &mut Node, &mut BackgroundColor)>,
) {
    if time.elapsed_secs_f64() - t.refreshed < 0.25 {
        return;
    }
    t.refreshed = time.elapsed_secs_f64();
    if t.graph.len() == 60 {
        t.graph.pop_front();
    }
    let peak = t.peak_ms;
    t.graph.push_back(peak);
    t.peak_ms = 0.0;
    let power = super::logging::power_state();
    t.thermal = power.thermal.into();
    for mut text in &mut texts.p3() {
        let value = if let Some(r) = &state.active {
            format!(
                "{} | {} {:.0}s | F1 cancel",
                ['A', 'B'][r.slot],
                if r.sampling_since.is_some() {
                    "Recording"
                } else {
                    "Settling"
                },
                time.elapsed_secs_f64() - r.sampling_since.unwrap_or(r.started)
            )
        } else {
            "Performance  |  F1".into()
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    if !state.open {
        return;
    }
    for mut text in &mut passes {
        text.0 = timing_details(&timings, *stamp);
    }
    let gpu_fresh = window.focused
        && timings
            .gpu_received
            .is_some_and(|t| t.elapsed().as_secs_f64() < 1.5);
    let recent_gpu: Vec<_> = timings
        .gpu
        .iter()
        .filter(|s| {
            gpu_fresh
                && !s.stamp.detailed
                && s.stamp.epoch == stamp.epoch
                && stamp.frame.saturating_sub(s.stamp.frame) < 240
        })
        .cloned()
        .collect();
    let recent_cpu: Vec<_> = timings
        .cpu
        .iter()
        .filter(|s| s.stamp.epoch == stamp.epoch && stamp.frame.saturating_sub(s.stamp.frame) < 240)
        .cloned()
        .collect();
    let mut timing_line = timing_summary(&recent_gpu, &recent_cpu);
    if let Ok(status) = upscaler.single() {
        timing_line += &status.description();
    }
    let s = stats(t.frames.iter().copied());
    for mut text in &mut texts.p0() {
        **text = format!(
            "{:.1} FPS | {:.2} ms median | p95 {:.2} ms\nThermal: {} | low power: {}\n{}",
            if s.mean > 0.0 { 1000.0 / s.mean } else { 0.0 },
            s.median,
            s.p95,
            power.thermal,
            power.low_power,
            timing_line
        );
    }
    for mut text in &mut texts.p1() {
        **text = format!(
            "3D: {}x{} | display: {}x{}\nMain-app elapsed median: {:.2} ms\nPipelines compiling: {} | loading pages: {}\nResident pages: {} | failed: {}\nTerrain: {} triangles / {} patches\nContact limited: {} | quality pending: {}\nObject LOD counts: {:?}\n{}",
            scene_size(&camera, upscaler.single().ok()).x,
            scene_size(&camera, upscaler.single().ok()).y,
            window.physical_width(),
            window.physical_height(),
            stats(t.app_times.iter().copied()).median,
            pending.0.load(Ordering::Relaxed),
            streaming.loading,
            streaming.resident,
            streaming.failed,
            terrain.triangles,
            terrain.patches,
            terrain.contact_limited,
            terrain.quality_pending,
            streaming.lod_counts,
            state.message
        );
    }
    for mut text in &mut texts.p2() {
        **text = comparison(&state);
    }
    for (bar, mut node, mut color) in &mut bars {
        let value = t.graph.iter().rev().nth(59 - bar.0).copied().unwrap_or(0.0);
        node.height = px((value as f32 / 33.33 * 64.0).clamp(1.0, 64.0));
        color.0 = if value > 16.67 {
            Color::srgb(0.95, 0.38, 0.3)
        } else if value > 8.34 {
            Color::srgb(0.9, 0.68, 0.25)
        } else {
            Color::srgb(0.2, 0.75, 0.65)
        };
    }
}
fn visibility(
    state: Res<PanelState>,
    settings: Res<AuditSettings>,
    mut nodes: Query<
        (
            &mut Node,
            Option<&PanelRoot>,
            Option<&Page>,
            Option<&engine::DiagnosticOverlay>,
        ),
        Or<(With<PanelRoot>, With<Page>, With<engine::DiagnosticOverlay>)>,
    >,
    mut tabs: Query<(&Action, &mut BackgroundColor)>,
) {
    for (mut node, root, page, overlay) in &mut nodes {
        let shown = if root.is_some() {
            Some(state.open)
        } else if let Some(page) = page {
            Some(page.0 == state.tab)
        } else if overlay.is_some() {
            Some(settings.overlays && !state.recording())
        } else {
            None
        };
        if let Some(shown) = shown {
            let display = if shown { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
    }
    for (action, mut color) in &mut tabs {
        if let Action::Tab(tab) = action {
            let desired = if *tab == state.tab {
                Color::srgb(0.16, 0.35, 0.42)
            } else {
                button_color()
            };
            if color.0 != desired {
                color.0 = desired;
            }
        }
    }
}
const TIMING_KINDS: [Kind; 12] = [
    Kind::Streaming,
    Kind::Terrain,
    Kind::Vegetation,
    Kind::Simulation,
    Kind::Other,
    Kind::Extract,
    Kind::Prepare,
    Kind::Encode,
    Kind::Submit,
    Kind::Acquire,
    Kind::RenderCall,
    Kind::Graph,
];
fn in_capture(sample: timing::Stamp, start: timing::Stamp, end: timing::Stamp) -> bool {
    sample.epoch == start.epoch && sample.frame > start.frame && sample.frame <= end.frame
}
fn timing_summary(gpu: &[timing::GpuSample], cpu: &[timing::CpuSample]) -> String {
    let omitted: usize = gpu.iter().map(|s| s.invalid_scopes).sum();
    let gpu_text = if gpu.len() < 5 {
        "GPU: waiting / unavailable".into()
    } else {
        let s = stats(gpu.iter().map(|s| s.elapsed_ms));
        format!(
            "GPU render: {:.2} ms | p95 {:.2} | {} samples",
            s.median,
            s.p95,
            gpu.len()
        )
    };
    if cpu.is_empty() {
        return format!("{gpu_text}\nCPU timings: waiting / unavailable\n");
    }
    let main = duration_stats(cpu.iter().filter(|s| !s.render).map(timing::cpu_main_ms));
    let render = || cpu.iter().filter(|s| s.render);
    let prep = duration_stats(render().map(|s| s.work[Kind::Prepare as usize]));
    let acquire = duration_stats(render().map(|s| s.work[Kind::Acquire as usize]));
    let submit = duration_stats(render().map(|s| s.work[Kind::Submit as usize]));
    let tail = duration_stats(render().filter_map(timing::render_tail_ms));
    format!(
        "{gpu_text}\nCPU work: main {:.2} / render prep {:.2} ms\nAcquire {:.2} / submit {:.2} / present+readback {:.2} ms\n{}",
        main.median,
        prep.median,
        acquire.median,
        submit.median,
        tail.median,
        if omitted > 0 {
            format!("{omitted} invalid detailed scope(s) omitted.\n")
        } else {
            String::new()
        }
    )
}
fn timing_details(history: &timing::History, stamp: timing::Stamp) -> String {
    let mut out = format!(
        "{}\nDropped / invalid samples: {}\n",
        history.status, history.dropped
    );
    let latest = history.gpu.iter().rev().find(|s| {
        s.stamp.detailed
            && s.stamp.epoch == stamp.epoch
            && stamp.frame.saturating_sub(s.stamp.frame) < 240
    });
    if let Some(gpu) = latest {
        out += "GPU diagnostic spans (ms, includes probe overhead):\n";
        if gpu.invalid_scopes > 0 {
            out += &format!("{} invalid scope(s) omitted.\n", gpu.invalid_scopes);
        }
        let mut scopes = gpu.scopes.clone();
        scopes.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (name, ms) in scopes.iter().take(8) {
            out += &format!("{ms:.2}  {}\n", display_name(name));
        }
    }
    if !stamp.detailed {
        out += "Enable GPU pass timings above for a breakdown (adds probe overhead).\n";
    } else {
        out += "Probes run every 60 frames and can reduce performance. Their totals are excluded from the normal GPU figure. A/B captures pause probes.\n";
    }
    let recent: Vec<_> = history
        .cpu
        .iter()
        .filter(|s| s.stamp.epoch == stamp.epoch && stamp.frame.saturating_sub(s.stamp.frame) < 240)
        .collect();
    out += "\nCPU categories (median system ms; parallel work may overlap):\n";
    for kind in [
        Kind::Streaming,
        Kind::Terrain,
        Kind::Vegetation,
        Kind::Simulation,
        Kind::Other,
        Kind::Extract,
        Kind::Prepare,
        Kind::Encode,
    ] {
        let render = !timing::MAIN_KINDS.contains(&kind);
        let ms = duration_stats(
            recent
                .iter()
                .filter(|s| s.render == render)
                .map(|s| s.work[kind as usize]),
        )
        .median;
        out += &format!("{ms:.2}  {}\n", kind.label());
    }
    out += "CPU systems (latest frames, ms):\n";
    for s in history
        .cpu
        .iter()
        .rev()
        .filter(|s| s.stamp.epoch == stamp.epoch)
        .take(2)
    {
        for (name, ms) in s.systems.iter().take(4) {
            out += &format!("{ms:.2}  {}\n", display_name(name));
        }
    }
    out
}
fn scene_size(camera: &Camera, status: Option<&upscaling::UpscaleStatus>) -> UVec2 {
    status.filter(|s| s.input_size != UVec2::ZERO).map_or_else(
        || camera.physical_viewport_size().unwrap_or_default(),
        |s| s.input_size,
    )
}
fn display_name(name: &str) -> &str {
    if name.contains("yarra_upscaling::output::draw") {
        return "Tone map + output";
    }
    if name.contains("temporal::resolve") {
        return "Temporal reconstruction";
    }
    if name.contains("temporal::initialize_motion") {
        return "Scene motion preparation";
    }
    if name.contains("temporal::prepare_previous") {
        return "Grass previous wind pose";
    }
    if name.contains("temporal::draw") {
        return "Grass colour + motion";
    }
    if name.contains("blade_preparation") {
        return "Grass generation";
    }
    if name.contains("msaa_store::opaque_pass") {
        return "Opaque terrain / objects / grass";
    }
    if name.contains("per_view_shadow_pass") {
        return "Shadows";
    }
    if name.contains("bloom::bloom") {
        return "Bloom";
    }
    name.rsplit("::").next().unwrap_or(name)
}
fn csv_text(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

fn export(state: &PanelState) -> std::io::Result<std::path::PathBuf> {
    if state.slots.iter().all(Option::is_none) {
        return Err(std::io::Error::other("Capture A or B first."));
    }
    let root = std::env::var_os("YARRA_PERFORMANCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "tmp/performance".into());
    export_to(state, &root)
}
fn export_to(state: &PanelState, root: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = root.join(format!("comparison-{stamp}"));
    std::fs::create_dir_all(&dir)?;
    let mut report = comparison(state);
    report += "\nGPU timestamps exclude presentation and uploads before rendering; sampled scopes include instrumentation overhead. CPU system durations may overlap; they are not CPU utilization. Main-app elapsed is not CPU busy time. These short captures do not establish sustained thermal performance.\n";
    for (i, slot) in state.slots.iter().enumerate() {
        if let Some(c) = slot {
            report += &format!(
                "\n{} context:\n{}\nSettings:\n{:#?}\n",
                ['A', 'B'][i],
                c.context,
                c.settings
            );
            let mut csv = String::from("frame,frame_interval_ms,main_app_elapsed_ms\n");
            for (frame, (interval, app)) in c.frames.iter().zip(&c.app_times).enumerate() {
                csv += &format!("{frame},{interval:.5},{app:.5}\n");
            }
            std::fs::write(dir.join(format!("{}.csv", ['A', 'B'][i])), csv)?;
            let mut gpu_csv = String::from("source_frame,settings_epoch,gpu_elapsed_ms,scope\n");
            for s in &c.gpu {
                gpu_csv += &format!(
                    "{},{},{:.6},render_total\n",
                    s.stamp.frame, s.stamp.epoch, s.elapsed_ms
                );
                for (name, ms) in &s.scopes {
                    gpu_csv += &format!(
                        "{},{},{:.6},{}\n",
                        s.stamp.frame,
                        s.stamp.epoch,
                        ms,
                        csv_text(name)
                    );
                }
            }
            std::fs::write(dir.join(format!("{}-gpu.csv", ['A', 'B'][i])), gpu_csv)?;
            let mut cpu_csv =
                String::from("source_frame,settings_epoch,domain,category,elapsed_ms\n");
            for s in &c.cpu {
                for kind in TIMING_KINDS {
                    cpu_csv += &format!(
                        "{},{},{},{},{:.6}\n",
                        s.stamp.frame,
                        s.stamp.epoch,
                        if s.render { "render" } else { "main" },
                        kind.label(),
                        s.work[kind as usize]
                    );
                }
            }
            std::fs::write(dir.join(format!("{}-cpu.csv", ['A', 'B'][i])), cpu_csv)?;
        }
    }
    std::fs::write(dir.join("report.txt"), report)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recording_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<FramePacing>()
            .init_resource::<Telemetry>()
            .init_resource::<AuditSettings>()
            .init_resource::<PendingPipelines>()
            .init_resource::<timing::History>()
            .init_resource::<timing::Stamp>()
            .init_resource::<engine::StreamingStats>()
            .init_resource::<engine::TerrainLodStats>()
            .init_resource::<engine::AtmosphereState>()
            .init_resource::<PanelState>()
            .add_systems(Update, sample);
        app.world_mut().spawn((
            Window {
                focused: true,
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::default(),
            WorldViewCamera,
        ));
        app.world_mut()
            .resource_mut::<AuditSettings>()
            .controls_locked = true;
        app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(60);
        app.world_mut().resource_mut::<PanelState>().active = Some(Recording {
            slot: 0,
            started: 0.0,
            sampling_since: None,
            timing_start: timing::Stamp::default(),
            was_locked: false,
            was_detailed: true,
            capture: Capture {
                settings: AuditSettings::default(),
                frame_rate: FrameRate::new(60),
                frames: vec![],
                app_times: vec![],
                thermal_start: String::new(),
                thermal_end: String::new(),
                context: String::new(),
                upscaler: None,
                camera: Mat4::IDENTITY,
                phase: 0.0,
                viewport: UVec2::ZERO,
                warnings: vec![],
                gpu: vec![],
                cpu: vec![],
            },
        });
        app
    }
    fn advance(app: &mut App, seconds: f64) {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_secs_f64(seconds));
        app.update();
    }
    #[test]
    fn capture_rejects_an_external_fps_change() {
        let mut app = recording_app();
        advance(&mut app, 2.1);
        app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(30);
        advance(&mut app, 0.1);
        let panel = app.world().resource::<PanelState>();
        assert!(!panel.recording());
        assert!(panel.slots[0].is_none());
        assert!(panel.message.contains("FPS limit changed"));
        assert!(!app.world().resource::<AuditSettings>().controls_locked);
    }

    #[test]
    fn capture_waits_for_upscaling_and_flags_a_backend_change() {
        let mut app = recording_app();
        let camera = app
            .world_mut()
            .query_filtered::<Entity, With<WorldViewCamera>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(camera)
            .insert(upscaling::UpscaleStatus::default());
        advance(&mut app, 2.1);
        assert!(
            app.world()
                .resource::<PanelState>()
                .active
                .as_ref()
                .unwrap()
                .sampling_since
                .is_none()
        );
        app.world_mut()
            .get_mut::<upscaling::UpscaleStatus>(camera)
            .unwrap()
            .active = Some(upscaling::UpscaleMethod::Linear);
        advance(&mut app, 0.1);
        assert!(
            app.world()
                .resource::<PanelState>()
                .active
                .as_ref()
                .unwrap()
                .sampling_since
                .is_some()
        );
        app.world_mut()
            .get_mut::<upscaling::UpscaleStatus>(camera)
            .unwrap()
            .active = Some(upscaling::UpscaleMethod::MetalFxSpatial);
        advance(&mut app, 0.1);
        let recording = app
            .world()
            .resource::<PanelState>()
            .active
            .as_ref()
            .unwrap();
        assert!(
            recording
                .capture
                .warnings
                .iter()
                .any(|w| w.contains("upscaler"))
        );
        assert_eq!(
            recording.capture.upscaler.as_ref().unwrap().active,
            Some(upscaling::UpscaleMethod::Linear)
        );
    }

    #[test]
    fn capture_is_bounded_and_restores_input_lock() {
        let mut app = recording_app();
        advance(&mut app, 2.1);
        assert!(
            app.world()
                .resource::<PanelState>()
                .active
                .as_ref()
                .unwrap()
                .sampling_since
                .is_some()
        );
        advance(&mut app, 5.0);
        assert!(app.world().resource::<PanelState>().recording());
        advance(&mut app, 5.1);
        let panel = app.world().resource::<PanelState>();
        assert!(!panel.recording());
        assert!(panel.open);
        assert_eq!(panel.slots[0].as_ref().unwrap().frames.len(), 2);
        assert!(!app.world().resource::<AuditSettings>().controls_locked);
        assert!(app.world().resource::<AuditSettings>().gpu_pass_timings);
    }
    #[test]
    fn capture_aborts_if_loading_never_settles_or_focus_is_lost() {
        let mut app = recording_app();
        app.world_mut()
            .resource_mut::<engine::StreamingStats>()
            .loading = 1;
        advance(&mut app, MAX_SETTLE_SECONDS + 0.1);
        assert!(!app.world().resource::<PanelState>().recording());
        assert!(app.world().resource::<PanelState>().slots[0].is_none());
        assert!(!app.world().resource::<AuditSettings>().controls_locked);
        assert!(app.world().resource::<AuditSettings>().gpu_pass_timings);
        let mut app = recording_app();
        advance(&mut app, 2.1);
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        advance(&mut app, 0.1);
        assert!(!app.world().resource::<PanelState>().recording());
        assert!(
            app.world()
                .resource::<PanelState>()
                .message
                .contains("lost focus")
        );
        assert!(!app.world().resource::<AuditSettings>().controls_locked);
    }

    #[test]
    fn restore_and_export_use_the_captured_configuration() {
        let mut app = recording_app();
        *app.world_mut().resource_mut::<timing::Stamp>() = timing::Stamp {
            frame: 100,
            epoch: 7,
            ..default()
        };
        advance(&mut app, 2.1);
        *app.world_mut().resource_mut::<timing::Stamp>() = timing::Stamp {
            frame: 200,
            epoch: 7,
            ..default()
        };
        for (frame, epoch, ms, detailed) in [
            (90, 7, 9.0, false),
            (102, 6, 99.0, false),
            (106, 7, 2.5, false),
            (120, 7, 200.0, true),
            (201, 7, 30.0, false),
        ] {
            app.world_mut()
                .resource_mut::<timing::History>()
                .gpu
                .push_back(timing::GpuSample {
                    stamp: timing::Stamp {
                        frame,
                        epoch,
                        detailed,
                    },
                    elapsed_ms: ms,
                    invalid_scopes: 0,
                    scopes: vec![("cloud, \"test\"".into(), 0.5)],
                });
        }
        advance(&mut app, 10.1);
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<bevy::input::mouse::AccumulatedMouseScroll>()
            .add_systems(Update, actions.before(sample));
        {
            let mut settings = app.world_mut().resource_mut::<AuditSettings>();
            settings.hide_objects = true;
            settings.clouds = engine::CloudQuality::Off;
        }
        app.world_mut()
            .spawn((Interaction::Pressed, Action::Restore(0)));
        app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(120);
        app.update();
        assert_eq!(app.world().resource::<FramePacing>().rate.fps(), 60);
        let settings = app.world().resource::<AuditSettings>();
        assert!(!settings.hide_objects);
        assert_eq!(
            settings.clouds,
            app.world().resource::<PanelState>().slots[0]
                .as_ref()
                .unwrap()
                .settings
                .clouds
        );
        let root = std::env::temp_dir().join(format!(
            "yarra-performance-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = export_to(app.world().resource::<PanelState>(), &root).unwrap();
        let report = std::fs::read_to_string(dir.join("report.txt")).unwrap();
        assert!(!report.contains("Settings changed:"));
        assert!(report.contains("3D viewport:"));
        assert!(report.contains("hide_objects: false"));
        let frames = std::fs::read_to_string(dir.join("A.csv")).unwrap();
        assert!(frames.starts_with("frame,frame_interval_ms,main_app_elapsed_ms"));
        assert_eq!(frames.lines().count(), 2);
        let gpu = std::fs::read_to_string(dir.join("A-gpu.csv")).unwrap();
        assert!(gpu.contains("106,7,2.500000,render_total"));
        assert!(gpu.contains("\"cloud, \"\"test\"\"\""));
        assert_eq!(
            gpu.lines().count(),
            3,
            "only normal, in-range samples from this settings epoch are captured"
        );
        assert!(dir.join("A-cpu.csv").exists());
        assert!(!dir.join("B.csv").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn percentiles_use_frame_intervals_and_ignore_invalid_samples() {
        let s = stats([8.0, 8.0, 10.0, 30.0, f64::NAN, 0.0]);
        assert_eq!(s.median, 8.0);
        assert_eq!(s.p95, 30.0);
        assert_eq!(s.mean, 14.0);
        assert_eq!(duration_stats([0.0, 0.0, 2.0, f64::NAN]).median, 0.0);
    }
    #[test]
    fn comparison_surfaces_multiple_changes() {
        let mut capture = Capture {
            settings: AuditSettings::default(),
            frame_rate: default(),
            frames: vec![8.0; 10],
            app_times: vec![2.0; 10],
            thermal_start: "nominal".into(),
            thermal_end: "nominal".into(),
            context: String::new(),
            upscaler: None,
            camera: Mat4::IDENTITY,
            phase: 0.5,
            viewport: UVec2::ZERO,
            warnings: vec![],
            gpu: vec![],
            cpu: vec![],
        };
        capture.gpu = (1..=5)
            .map(|frame| timing::GpuSample {
                stamp: timing::Stamp { frame, ..default() },
                elapsed_ms: 1.5,
                scopes: vec![],
                invalid_scopes: 0,
            })
            .collect();
        let mut b = capture.clone();
        for sample in &mut b.gpu {
            sample.elapsed_ms = 2.2;
        }
        b.settings.clouds = engine::CloudQuality::Off;
        b.frame_rate = FrameRate::new(60);
        b.settings.hide_objects = true;
        b.thermal_start = "fair".into();
        let state = PanelState {
            slots: [Some(capture), Some(b)],
            ..default()
        };
        let report = comparison(&state);
        assert!(report.contains("Clouds:"));
        assert!(report.contains("FPS limit: Follow display -> FPS limit: 60"));
        assert!(report.contains("Object draws:"));
        assert!(report.contains("Conditions differ"));
        assert!(report.contains("B - A GPU median: +0.70 ms"));
        assert!(report.contains("B - A median: +0.00 ms"));
    }
}
