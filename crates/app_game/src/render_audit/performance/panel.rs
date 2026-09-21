//! F1 presentation and input routing. Recording and report generation live elsewhere.
use super::{
    capture::CaptureSession,
    report,
    telemetry::{self, PendingPipelines, Telemetry},
};
use crate::{
    frame_pacing::FramePacing,
    render_audit::{AuditPanel, Control, font, timing},
    runtime_settings::{RuntimeSettings, RuntimeSettingsInit},
};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use engine::WorldViewCamera;

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
pub(crate) struct PanelState {
    open: bool,
    tab: Tab,
}
impl Default for PanelState {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Overview,
        }
    }
}
pub(super) fn install(app: &mut App) {
    let open = app
        .world()
        .resource::<crate::launch::LaunchOptions>()
        .panel_open;
    app.insert_resource(PanelState { open, ..default() })
        .add_systems(PostStartup, initialize.after(RuntimeSettingsInit))
        .add_systems(
            Update,
            (
                actions,
                telemetry::sample,
                super::capture::sample.pipe(capture_finished),
            )
                .chain()
                .before(crate::render_audit::buttons),
        )
        .add_systems(PostUpdate, (refresh, visibility).chain());
}

fn initialize(
    mut commands: Commands,
    s: Res<RuntimeSettings>,
    pacing: Res<FramePacing>,
    options: Res<crate::launch::LaunchOptions>,
) {
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
            (Tab::Advanced, "Diagnostic experiments. Compute only hides grass draws but keeps generation; Schedule only omits generation. Draw frozen locks controls. These settings do not change published content.", &[Control::Grass, Control::Scene, Control::Shading, Control::GroundMaterial, Control::Prepass, Control::Counters, Control::GpuPassTimings, Control::RenderPath, Control::Lock, Control::PageGizmos, Control::TerrainMacro, Control::ReloadCanopy][..]),
        ] {
            panel.spawn((Page(tab), page_node())).with_children(|page| {
                page.spawn((Text::new(note), font(12.0)));
                if tab == Tab::Advanced { page.spawn((Text::new(""), font(12.0), PassTimes)); }
                page.spawn(Node { flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), ..default() }).with_children(|row| {
                    for &control in controls {
                        if matches!(control, Control::GpuPassTimings) && (options.diagnostics != crate::launch::DiagnosticsMode::Full || options.gpu_off) { continue; }
                        row.spawn((Button, control, Node { width: px(218), ..button_node() }, BackgroundColor(button_color()))).with_child((Text::new(control.label(&s, pacing.rate)), font(13.0))); }
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
    mut session: ResMut<CaptureSession>,
    mut settings: ResMut<RuntimeSettings>,
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
        || (keys.just_pressed(KeyCode::Escape) && (state.open || session.recording()))
    {
        actions.push(Action::Toggle);
    }
    for action in actions {
        if matches!(action, Action::Toggle) {
            if session.cancel(&mut settings, "Capture cancelled; previous results kept.") {
                state.open = true;
            } else {
                state.open = !state.open;
            }
            continue;
        }
        if session.recording() {
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
                session.restore(slot, &mut settings, &mut pacing, time.elapsed_secs_f64());
            }
            Action::Capture(slot) => {
                if session.start(slot, &mut settings, pacing.rate, time.elapsed_secs_f64()) {
                    state.open = false;
                }
            }
            Action::Export => {
                session.message = match report::export(&session) {
                    Ok(path) => format!("Exported to {}", path.display()),
                    Err(error) => format!("Export failed: {error}"),
                };
            }
            Action::Toggle => unreachable!(),
        }
    }
}

/// Only presentation decides what finishing a capture does to the panel.
fn capture_finished(In(finished): In<bool>, mut state: ResMut<PanelState>) {
    if finished {
        state.open = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn refresh(
    canopy: Option<Res<engine::GroundCanopyTiles>>,
    vegetation: Res<vegetation_render::VegetationDiagnostics>,
    time: Res<Time<Real>>,
    (mut t, pending): (ResMut<Telemetry>, Res<PendingPipelines>),
    state: Res<PanelState>,
    session: Res<CaptureSession>,
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
    let power = crate::render_audit::logging::power_state();
    for mut text in &mut texts.p3() {
        let value = session.progress(time.elapsed_secs_f64()).map_or_else(
            || "Performance  |  F1".into(),
            |progress| {
                format!(
                    "{} | {} {:.0}s | F1 cancel",
                    ['A', 'B'][progress.slot],
                    if progress.sampling {
                        "Recording"
                    } else {
                        "Settling"
                    },
                    progress.elapsed
                )
            },
        );
        if text.0 != value {
            text.0 = value;
        }
    }
    if !state.open {
        return;
    }
    for mut text in &mut passes {
        text.0 = report::timing_details(&timings, *stamp);
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
    let mut timing_line = if timings.enabled {
        report::timing_summary(&recent_gpu, &recent_cpu)
    } else {
        format!("{}\n", timings.status)
    };
    if let Ok(status) = upscaler.single() {
        timing_line += &status.description();
    }
    let s = report::stats(t.frames.iter().copied());
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
    let grass = vegetation.snapshot();
    let (ready, total, jobs) = canopy.as_ref().map_or((0, 0, 0), |tiles| tiles.counts());
    let source_info = format!(
        "Canopy coverage: {ready}/{total} tiles | {jobs} jobs\nSource memory: {:.2} MiB decoded / {:.2} MiB estimated GPU\nGrass: {} source pages | {} work items | {} repacks\nGrass source buffers: {:.2} MiB | dropped: {:?} ({} GPU samples)",
        streaming.decoded_bytes as f64 / 1048576.0,
        streaming.gpu_bytes_estimate as f64 / 1048576.0,
        grass.source_pages,
        grass.source_work_items,
        grass.source_repacks,
        grass.source_buffer_capacity_bytes as f64 / 1048576.0,
        grass.capacity_dropped_instances,
        grass.gpu_samples
    );
    for mut text in &mut texts.p1() {
        **text = format!(
            "3D: {}x{} | display: {}x{}\nMain-app elapsed median: {:.2} ms\nPipelines compiling: {} | loading pages: {}\nResident pages: {} | failed: {}\nTerrain: {} triangles / {} patches\nContact limited: {} | quality pending: {}\nObject LOD counts: {:?}\n{source_info}\n{}",
            telemetry::scene_size(&camera, upscaler.single().ok()).x,
            telemetry::scene_size(&camera, upscaler.single().ok()).y,
            window.physical_width(),
            window.physical_height(),
            report::stats(t.app_times.iter().copied()).median,
            pending.count(),
            streaming.loading,
            streaming.resident,
            streaming.failed,
            terrain.triangles,
            terrain.patches,
            terrain.contact_limited,
            terrain.quality_pending,
            streaming.lod_counts,
            session.message
        );
    }
    for mut text in &mut texts.p2() {
        **text = report::comparison(&session);
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
    mut nodes: Query<
        (&mut Node, Option<&PanelRoot>, Option<&Page>),
        Or<(With<PanelRoot>, With<Page>)>,
    >,
    mut tabs: Query<(&Action, &mut BackgroundColor)>,
) {
    for (mut node, root, page) in &mut nodes {
        let shown = if root.is_some() {
            Some(state.open)
        } else {
            page.map(|page| page.0 == state.tab)
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

#[cfg(test)]
mod tests;
