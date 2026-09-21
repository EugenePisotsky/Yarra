//! Measurements keep their source-frame identity across the render thread and async readback.
//! System wrappers preserve access, conditions, ordering and deferred commands (Bevy 0.19).
#[path = "timing/gpu.rs"]
mod gpu;
use bevy::{
    ecs::{
        change_detection::{CheckChangeTicks, Tick},
        query::FilteredAccessSet,
        schedule::{InternedSystemSet, SystemWithAccess},
        system::{IntoSystem, RunSystemError, System, SystemStateFlags},
        world::{DeferredWorld, unsafe_world_cell::UnsafeWorldCell},
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        renderer::{RenderGraph, RenderGraphSystems},
    },
    utils::prelude::DebugName,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

const HISTORY: usize = 8192;
#[derive(Resource, ExtractResource, Clone, Copy, Debug, Default)]
pub(super) struct Stamp {
    pub frame: u32,
    pub epoch: u64,
    pub detailed: bool,
}
#[derive(Clone, Debug)]
pub(super) struct GpuSample {
    pub stamp: Stamp,
    pub elapsed_ms: f64,
    pub scopes: Vec<(String, f64)>,
    pub invalid_scopes: usize,
}
#[derive(Clone, Debug)]
pub(super) struct CpuSample {
    pub stamp: Stamp,
    pub render: bool,
    pub work: [f64; KINDS],
    pub systems: Vec<(String, f64)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum Kind {
    Streaming,
    Terrain,
    Vegetation,
    Simulation,
    Other,
    Extract,
    Prepare,
    Encode,
    Submit,
    Acquire,
    RenderCall,
    Graph,
}
pub(super) const KINDS: usize = 12;
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Streaming => "Streaming systems",
            Self::Terrain => "Terrain systems",
            Self::Vegetation => "Vegetation systems",
            Self::Simulation => "Simulation / animation",
            Self::Other => "Other main systems",
            Self::Extract => "Render extraction",
            Self::Prepare => "Render preparation",
            Self::Encode => "Render encoding",
            Self::Submit => "Queue submission",
            Self::Acquire => "Surface acquisition",
            Self::RenderCall => "Render call",
            Self::Graph => "Render graph",
        }
    }
}
pub(super) const MAIN_KINDS: [Kind; 5] = [
    Kind::Streaming,
    Kind::Terrain,
    Kind::Vegetation,
    Kind::Simulation,
    Kind::Other,
];
#[derive(Default)]
struct Bus {
    gpu: Vec<GpuSample>,
    cpu: Vec<CpuSample>,
    status: String,
    dropped: u64,
}
#[derive(Resource, Clone, Default)]
struct Bridge(Arc<Mutex<Bus>>);
#[derive(Resource)]
pub(super) struct History {
    pub enabled: bool,
    pub gpu_disabled: bool,
    pub gpu: VecDeque<GpuSample>,
    pub cpu: VecDeque<CpuSample>,
    pub status: String,
    pub dropped: u64,
    pub gpu_received: Option<Instant>,
}
impl Default for History {
    fn default() -> Self {
        Self {
            enabled: false,
            gpu_disabled: true,
            gpu: VecDeque::new(),
            cpu: VecDeque::new(),
            status: "CPU/GPU timing probes disabled. Restart with --diagnostics full for timings."
                .into(),
            dropped: 0,
            gpu_received: None,
        }
    }
}
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct CollectTimings;

struct Probe {
    name: String,
    kind: Kind,
    ns: AtomicU64,
}
#[derive(Resource, Default)]
struct Probes {
    entries: Vec<Arc<Probe>>,
    render: bool,
}
#[derive(Resource, Default)]
struct GraphTime(AtomicU64);

#[derive(Default)]
pub(super) struct TimingPlugin {
    pub log: bool,
    pub gpu_off: bool,
}
impl Plugin for TimingPlugin {
    fn build(&self, app: &mut App) {
        let bridge = Bridge::default();
        let logging = self.log;
        app.insert_resource(bridge.clone())
            .insert_resource(History {
                enabled: true,
                gpu_disabled: self.gpu_off,
                status: "Waiting for timing samples...".into(),
                ..default()
            })
            .init_resource::<Stamp>()
            .add_plugins(ExtractResourcePlugin::<Stamp>::default())
            .add_systems(First, receive)
            .add_systems(Update, log_timings.run_if(move || logging))
            .add_systems(Last, collect_cpu.in_set(CollectTimings));
        let render = app.sub_app_mut(RenderApp);
        render
            .insert_resource(bridge)
            .init_resource::<Stamp>()
            .init_resource::<GraphTime>()
            .add_systems(Render, collect_cpu.after(RenderSystems::PostCleanup));
        gpu::install(render, !self.gpu_off);
    }
    fn finish(&self, app: &mut App) {
        // All plugins have populated their schedules, and the opaque-pass replacement is in place.
        instrument(app.world_mut(), false);
        instrument(app.sub_app_mut(RenderApp).world_mut(), true);
    }
}
fn receive(bridge: Res<Bridge>, mut history: ResMut<History>) {
    let Ok(mut bus) = bridge.0.lock() else {
        return;
    };
    if !bus.gpu.is_empty() {
        history.gpu_received = Some(Instant::now());
    }
    for sample in bus.gpu.drain(..) {
        history.gpu.push_back(sample);
    }
    for sample in bus.cpu.drain(..) {
        history.cpu.push_back(sample);
    }
    while history.gpu.len() > HISTORY {
        history.gpu.pop_front();
    }
    while history.cpu.len() > HISTORY {
        history.cpu.pop_front();
    }
    history.status.clone_from(&bus.status);
    history.dropped = bus.dropped;
}
fn collect_cpu(
    stamp: Res<Stamp>,
    probes: Res<Probes>,
    bridge: Res<Bridge>,
    graph: Option<Res<GraphTime>>,
) {
    let mut sample = CpuSample {
        stamp: *stamp,
        render: probes.render,
        work: [0.0; KINDS],
        systems: Vec::new(),
    };
    for probe in &probes.entries {
        let ms = probe.ns.swap(0, Ordering::Relaxed) as f64 / 1_000_000.0;
        sample.work[probe.kind as usize] += ms;
        if ms >= 0.02 {
            sample.systems.push((probe.name.clone(), ms));
        }
    }
    if let Some(graph) = graph {
        sample.work[Kind::Graph as usize] = graph.0.swap(0, Ordering::Relaxed) as f64 / 1_000_000.0;
    }
    sample.systems.sort_by(|a, b| b.1.total_cmp(&a.1));
    sample.systems.truncate(8);
    if let Ok(mut bus) = bridge.0.lock() {
        if bus.cpu.len() < HISTORY {
            bus.cpu.push(sample);
        }
    }
}
fn classify(name: &str, schedule: &str, render: bool) -> Kind {
    if name.ends_with("::prepare_windows") {
        Kind::Acquire
    } else if name.ends_with("::render_system") {
        Kind::RenderCall
    } else if name.contains("submit_pending_command_buffers") {
        Kind::Submit
    } else if render && schedule.contains("ExtractSchedule") {
        Kind::Extract
    } else if render && schedule == "Render" {
        Kind::Prepare
    } else if render {
        Kind::Encode
    } else if name.contains("world_streaming::terrain_lod") || name.contains("yarra_terrain_render")
    {
        Kind::Terrain
    } else if name.contains("world_streaming") {
        Kind::Streaming
    } else if name.contains("vegetation") {
        Kind::Vegetation
    } else if name.contains("animation") || name.contains("character") || name.contains("motor") {
        Kind::Simulation
    } else {
        Kind::Other
    }
}
fn instrument(world: &mut World, render: bool) {
    let mut probes = Probes {
        render,
        ..default()
    };
    let mut schedules = world.resource_mut::<Schedules>();
    for (label, schedule) in schedules.iter_mut() {
        let label = format!("{label:?}");
        let wanted = if render {
            !["First", "RenderStartup", "RenderRecovery", "Main"].contains(&label.as_str())
        } else {
            [
                "PreUpdate",
                "FixedFirst",
                "FixedPreUpdate",
                "FixedUpdate",
                "FixedPostUpdate",
                "FixedLast",
                "Update",
                "PostUpdate",
            ]
            .contains(&label.as_str())
        };
        if !wanted {
            continue;
        }
        if schedule.graph().systems.is_empty() {
            continue;
        }
        assert!(
            !schedule.graph().systems.is_initialized(),
            "timing adapters must precede schedule initialization: {label}"
        );
        let mut root_sets = Vec::new();
        let keys: Vec<_> = schedule
            .graph()
            .systems
            .iter()
            .map(|(key, _, _)| key)
            .collect();
        for key in keys {
            let slot = schedule.graph_mut().systems.get_mut(key).unwrap();
            let name = slot.name().to_string();
            if name.contains(module_path!()) || name.contains("camera_driver") {
                continue;
            }
            let probe = Arc::new(Probe {
                kind: classify(&name, &label, render),
                name: name.clone(),
                ns: AtomicU64::new(0),
            });
            if render && label == "RenderGraph" {
                root_sets.extend(slot.default_system_sets());
            }
            // Deferred flags are populated by initialize, which has not run yet.
            let gpu = render && !["Render", "ExtractSchedule"].contains(&label.as_str());
            let original = std::mem::replace(
                slot,
                SystemWithAccess::new(Box::new(IntoSystem::into_system(|| {}))),
            );
            *slot = SystemWithAccess::new(Box::new(Measured {
                inner: original,
                probe: probe.clone(),
                gpu,
            }));
            probes.entries.push(probe);
        }
        for set in root_sets {
            schedule.configure_sets(gpu::FrameBegin.before(set));
        }
    }
    drop(schedules);
    world.insert_resource(probes);
}

/// Timing the call itself does not add schedule barriers or expand ECS resource access.
struct Measured {
    inner: SystemWithAccess,
    probe: Arc<Probe>,
    gpu: bool,
}
impl System for Measured {
    type In = ();
    type Out = ();
    fn name(&self) -> DebugName {
        self.inner.name()
    }
    fn system_type(&self) -> std::any::TypeId {
        self.inner.system_type()
    }
    fn flags(&self) -> SystemStateFlags {
        self.inner.flags()
    }
    unsafe fn run_unsafe(
        &mut self,
        input: (),
        world: UnsafeWorldCell,
    ) -> Result<(), RunSystemError> {
        let start = Instant::now();
        // SAFETY: identical access, flags and input to the wrapped system; no world access added.
        let result = unsafe { self.inner.run_unsafe(input, world) };
        if result.is_ok() {
            self.probe
                .ns
                .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        result
    }
    fn apply_deferred(&mut self, world: &mut World) {
        let previous = gpu::take_pending(DeferredWorld::from(&mut *world), self.gpu);
        let start = Instant::now();
        self.inner.apply_deferred(world);
        self.probe
            .ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        gpu::wrap_pending(DeferredWorld::from(&mut *world), previous, &self.probe.name);
    }
    fn queue_deferred(&mut self, mut world: DeferredWorld) {
        let previous = gpu::take_pending(world.reborrow(), self.gpu);
        let start = Instant::now();
        self.inner.queue_deferred(world.reborrow());
        self.probe
            .ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        gpu::wrap_pending(world, previous, &self.probe.name);
    }
    fn initialize(&mut self, world: &mut World) -> FilteredAccessSet {
        self.inner.initialize(world)
    }
    fn check_change_tick(&mut self, check: CheckChangeTicks) {
        self.inner.check_change_tick(check);
    }
    fn default_system_sets(&self) -> Vec<InternedSystemSet> {
        self.inner.default_system_sets()
    }
    fn get_last_run(&self) -> Tick {
        self.inner.get_last_run()
    }
    fn set_last_run(&mut self, tick: Tick) {
        self.inner.set_last_run(tick);
    }
}

pub(super) fn cpu_main_ms(sample: &CpuSample) -> f64 {
    MAIN_KINDS.iter().map(|k| sample.work[*k as usize]).sum()
}
pub(super) fn render_tail_ms(sample: &CpuSample) -> Option<f64> {
    let call = sample.work[Kind::RenderCall as usize];
    let graph = sample.work[Kind::Graph as usize];
    (call > 0.0 && graph > 0.0).then_some((call - graph).max(0.0))
}

fn log_timings(time: Res<Time<Real>>, history: Res<History>, mut last: Local<f64>) {
    if time.elapsed_secs_f64() - *last < 1.0 {
        return;
    }
    *last = time.elapsed_secs_f64();
    let gpu = history.gpu.iter().rev().find(|s| !s.stamp.detailed);
    let detail = history.gpu.iter().rev().find(|s| s.stamp.detailed);
    let render = history
        .cpu
        .iter()
        .rev()
        .find(|s| s.work[Kind::RenderCall as usize] > 0.0);
    let main = history.cpu.iter().rev().find(|s| cpu_main_ms(s) > 0.0);
    warn!(
        "PERFORMANCE_TIMING gpu_ms={:?} gpu_frame={:?} probe_frame={:?} probe_gpu_ms={:?} scopes={:?} main_work_ms={:?} render_prep_ms={:?} acquire_ms={:?} submit_ms={:?} tail_ms={:?} dropped={} status={}",
        gpu.map(|s| s.elapsed_ms),
        gpu.map(|s| s.stamp.frame),
        detail.map(|s| s.stamp.frame),
        detail.map(|s| s.elapsed_ms),
        detail.map(|s| &s.scopes),
        main.map(cpu_main_ms),
        render.map(|s| s.work[Kind::Prepare as usize]),
        render.map(|s| s.work[Kind::Acquire as usize]),
        render.map(|s| s.work[Kind::Submit as usize]),
        render.and_then(render_tail_ms),
        history.dropped,
        history.status
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Count(u32);
    #[derive(Component)]
    struct Spawned;
    fn create(mut commands: Commands, mut count: ResMut<Count>) {
        count.0 += 1;
        commands.spawn(Spawned);
    }
    fn observe(query: Query<&Spawned>, mut count: ResMut<Count>) {
        assert_eq!(query.iter().count(), 1);
        count.0 += 10;
    }
    fn skipped(mut count: ResMut<Count>) {
        count.0 += 100;
    }

    #[test]
    fn adapters_preserve_conditions_order_and_deferred_commands() {
        let mut app = App::new();
        app.init_resource::<Count>()
            .add_systems(
                Update,
                (
                    IntoSystem::into_system(create).with_name("test::create"),
                    IntoSystem::into_system(observe).with_name("test::observe"),
                )
                    .chain(),
            )
            .add_systems(
                Update,
                IntoSystem::into_system(skipped)
                    .with_name("test::skipped")
                    .run_if(|| false),
            );
        instrument(app.world_mut(), false);
        app.update();
        assert_eq!(app.world().resource::<Count>().0, 11);
        let probes = app.world().resource::<Probes>();
        assert!(
            probes
                .entries
                .iter()
                .find(|p| p.name.ends_with("::create"))
                .unwrap()
                .ns
                .load(Ordering::Relaxed)
                > 0
        );
        assert_eq!(
            probes
                .entries
                .iter()
                .find(|p| p.name.ends_with("::skipped"))
                .unwrap()
                .ns
                .load(Ordering::Relaxed),
            0
        );
    }

    #[test]
    fn presentation_related_work_is_not_classed_as_render_preparation() {
        assert_eq!(
            classify("bevy_render::view::window::prepare_windows", "Render", true),
            Kind::Acquire
        );
        assert_eq!(
            classify("bevy_render::renderer::render_system", "Render", true),
            Kind::RenderCall
        );
        assert_eq!(
            classify(
                "bevy_render::renderer::submit_pending_command_buffers",
                "RenderGraph",
                true
            ),
            Kind::Submit
        );
        assert_eq!(classify("prepare_meshes", "Render", true), Kind::Prepare);
        assert_eq!(
            classify(
                "yarra_engine::world_vegetation::canopy::sync",
                "Update",
                false
            ),
            Kind::Vegetation
        );
    }
}
