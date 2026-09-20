//! Stage-boundary timestamps, never CommandEncoder::write_timestamp (macOS workaround).
//! Fixed readback ring: no waits, no growth, no reuse until map completion and unmap.
use super::*;
use bevy::{
    ecs::world::DeferredWorld,
    render::{
        GpuResourceAppExt,
        render_resource::*,
        renderer::{PendingCommandBuffers, RenderDevice, RenderQueue},
    },
};
use std::sync::atomic::{AtomicU8, Ordering};
use wgpu::{CommandBuffer, ComputePassTimestampWrites, QuerySet, QuerySetDescriptor, QueryType};
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct FrameBegin;
const QUERIES: u32 = 128;
const BYTES: u64 = QUERIES as u64 * 8;
const SLOTS: usize = 4;
const SAMPLE_EVERY: u32 = 6;
struct Slot {
    queries: QuerySet,
    resolve: Buffer,
    readback: Buffer,
    state: Arc<AtomicU8>, // 0 free, 1 GPU pending, 2 resolve ready, 3 mapping, 4 mapped, 5 failed
    stamp: Stamp,
    scopes: Vec<String>,
    used: u32,
}
#[derive(Resource)]
struct Timer {
    slots: Vec<Slot>,
    active: Option<usize>,
    submitted: Option<usize>,
    period_ns: f64,
    supported: bool,
    marker_pipeline: Option<ComputePipeline>,
    graph_start: Option<Instant>,
}
impl FromWorld for Timer {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let supported = device
            .features()
            .contains(bevy::render::settings::WgpuFeatures::TIMESTAMP_QUERY)
            && !std::env::args_os().any(|a| a == "--gpu-timing-off");
        let slots = if supported {
            (0..SLOTS)
                .map(|_| Slot {
                    queries: device.wgpu_device().create_query_set(&QuerySetDescriptor {
                        label: Some("performance timestamps"),
                        ty: QueryType::Timestamp,
                        count: QUERIES,
                    }),
                    resolve: device.create_buffer(&BufferDescriptor {
                        label: Some("performance query resolve"),
                        size: BYTES,
                        usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    }),
                    readback: device.create_buffer(&BufferDescriptor {
                        label: Some("performance timestamp readback"),
                        size: BYTES,
                        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    }),
                    state: Arc::new(AtomicU8::new(0)),
                    stamp: default(),
                    scopes: vec![],
                    used: 2,
                })
                .collect()
        } else {
            vec![]
        };
        let period_ns = world.resource::<RenderQueue>().get_timestamp_period() as f64;
        let marker_pipeline = supported.then(|| {
            let shader = device
                .wgpu_device()
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("timestamp marker"),
                    source: wgpu::ShaderSource::Wgsl(
                        "@compute @workgroup_size(1) fn main() {}".into(),
                    ),
                });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("timestamp marker"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: default(),
                cache: None,
            })
        });
        let status = if supported {
            "GPU timestamps: waiting for completed samples"
        } else {
            "GPU timestamps unsupported or --gpu-timing-off"
        };
        world.resource::<Bridge>().0.lock().unwrap().status = status.into();
        info!("PERFORMANCE_TIMING supported={supported} timestamp_period_ns={period_ns}");
        Self {
            slots,
            active: None,
            submitted: None,
            period_ns,
            supported,
            marker_pipeline,
            graph_start: None,
        }
    }
}
pub(super) fn install(render: &mut SubApp) {
    render
        .init_gpu_resource::<Timer>()
        .add_systems(
            RenderGraph,
            begin.in_set(FrameBegin).before(RenderGraphSystems::Begin),
        )
        .add_systems(
            RenderGraph,
            resolve
                .after(RenderGraphSystems::Render)
                .before(RenderGraphSystems::Submit),
        )
        .add_systems(RenderGraph, finish.after(RenderGraphSystems::Finish));
}
fn marker(
    device: &RenderDevice,
    pipeline: &ComputePipeline,
    queries: &QuerySet,
    index: u32,
) -> CommandBuffer {
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("performance stage marker"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("performance stage timestamp"),
            timestamp_writes: Some(ComputePassTimestampWrites {
                query_set: queries,
                beginning_of_pass_write_index: index.is_multiple_of(2).then_some(index),
                end_of_pass_write_index: (!index.is_multiple_of(2)).then_some(index),
            }),
        });
        // Metal does not reliably sample empty encoders. A single no-op invocation makes
        // this a real stage without touching scene resources or requiring a blocking wait.
        pass.set_pipeline(pipeline);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.finish()
}
fn begin(
    mut timer: ResMut<Timer>,
    stamp: Res<Stamp>,
    bridge: Res<Bridge>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    timer.graph_start = Some(Instant::now());
    timer.active = None;
    let period_ns = timer.period_ns;
    for slot in &mut timer.slots {
        match slot.state.load(Ordering::Acquire) {
            2 => {
                // Metal counter resolves can race stage-end samples in the same submission.
                // Resolve only after the original submission completes, without waiting on it.
                let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                    label: Some("performance timestamp resolve"),
                });
                encoder.resolve_query_set(&slot.queries, 0..slot.used, &slot.resolve, 0);
                encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, BYTES);
                queue.submit([encoder.finish()]);
                slot.state.store(3, Ordering::Release);
                let state = slot.state.clone();
                slot.readback
                    .slice(..)
                    .map_async(MapMode::Read, move |result| {
                        state.store(if result.is_ok() { 4 } else { 5 }, Ordering::Release);
                    });
            }
            4 => {
                let data = slot.readback.slice(..).get_mapped_range();
                let times: Vec<u64> = data
                    .chunks_exact(8)
                    .take(slot.used as usize)
                    .map(|v| u64::from_le_bytes(v.try_into().unwrap()))
                    .collect();
                let sample = decode(slot.stamp, &times, &slot.scopes, period_ns);
                drop(data);
                slot.readback.unmap();
                slot.state.store(0, Ordering::Release);
                let mut bus = bridge.0.lock().unwrap();
                if let Some(sample) = sample {
                    if bus.gpu.len() < HISTORY {
                        bus.gpu.push(sample);
                    }
                    bus.status = "GPU render timestamps (sampled every 6 frames)".into();
                } else {
                    bus.dropped += 1;
                    bus.status = "GPU timestamp sample invalid; no estimate substituted".into();
                }
            }
            5 => {
                slot.readback.unmap();
                slot.state.store(0, Ordering::Release);
                bridge.0.lock().unwrap().dropped += 1;
            }
            _ => {}
        }
    }
    if !timer.supported || !stamp.frame.is_multiple_of(SAMPLE_EVERY) {
        return;
    }
    if let Some(index) = timer
        .slots
        .iter()
        .position(|s| s.state.load(Ordering::Acquire) == 0)
    {
        let slot = &mut timer.slots[index];
        slot.stamp = *stamp;
        slot.scopes.clear();
        slot.used = 2;
        timer.active = Some(index);
    } else {
        bridge.0.lock().unwrap().dropped += 1;
    }
}
/// Isolate exactly the command buffers of a system, preserving the previous pending order.
pub(super) fn take_pending(mut world: DeferredWorld, enabled: bool) -> Option<Vec<CommandBuffer>> {
    if !enabled
        || world
            .get_resource::<Timer>()
            .is_none_or(|t| t.active.is_none_or(|i| !t.slots[i].stamp.detailed))
    {
        return None;
    }
    Some(world.resource_mut::<PendingCommandBuffers>().take())
}
pub(super) fn wrap_pending(
    mut world: DeferredWorld,
    previous: Option<Vec<CommandBuffer>>,
    name: &str,
) {
    let Some(previous) = previous else {
        return;
    };
    let commands = world.resource_mut::<PendingCommandBuffers>().take();
    let device = world.resource::<RenderDevice>().clone();
    let pair = if commands.is_empty() {
        None
    } else {
        let mut timer = world.resource_mut::<Timer>();
        let pipeline = timer.marker_pipeline.as_ref().unwrap().clone();
        let slot = timer.active.map(|i| &mut timer.slots[i]).unwrap();
        if slot.used + 2 > QUERIES {
            None
        } else {
            let start = marker(&device, &pipeline, &slot.queries, slot.used);
            let end = marker(&device, &pipeline, &slot.queries, slot.used + 1);
            slot.used += 2;
            slot.scopes.push(name.to_owned());
            Some((start, end))
        }
    };
    let mut pending = world.resource_mut::<PendingCommandBuffers>();
    pending.push(previous);
    if let Some((start, end)) = pair {
        pending.push([start]);
        pending.push(commands);
        pending.push([end]);
    } else {
        pending.push(commands);
    }
}
fn resolve(
    mut timer: ResMut<Timer>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let Some(index) = timer.active.take() else {
        return;
    };
    let slot = &timer.slots[index];
    let pipeline = timer.marker_pipeline.as_ref().unwrap();
    // All renderer commands are encoded before queue submission. Presentation is outside this span.
    let commands = pending.take();
    pending.push([marker(&device, pipeline, &slot.queries, 0)]);
    pending.push(commands);
    pending.push([marker(&device, pipeline, &slot.queries, 1)]);
    timer.submitted = Some(index);
}
fn finish(mut timer: ResMut<Timer>, graph: Res<GraphTime>, queue: Res<RenderQueue>) {
    if let Some(start) = timer.graph_start.take() {
        graph
            .0
            .store(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    let Some(index) = timer.submitted.take() else {
        return;
    };
    let slot = &timer.slots[index];
    slot.state.store(1, Ordering::Release);
    let state = slot.state.clone();
    queue.on_submitted_work_done(move || state.store(2, Ordering::Release));
}
fn decode(stamp: Stamp, times: &[u64], labels: &[String], period_ns: f64) -> Option<GpuSample> {
    if times.len() != 2 + labels.len() * 2 || !period_ns.is_finite() || period_ns <= 0.0 {
        return None;
    }
    let duration = |a: u64, b: u64| -> Option<f64> {
        let ms = b.checked_sub(a)? as f64 * period_ns / 1_000_000.0;
        (a > 0 && ms.is_finite() && (0.0..60_000.0).contains(&ms)).then_some(ms)
    };
    let elapsed_ms = duration(times[0], times[1])?;
    if elapsed_ms == 0.0 {
        return None;
    }
    let mut scopes = vec![];
    let mut invalid_scopes = 0;
    for (i, name) in labels.iter().enumerate() {
        let start = times[2 + i * 2];
        let end = times[3 + i * 2];
        if start < times[0] || end > times[1] || duration(start, end).is_none() {
            invalid_scopes += 1;
            continue;
        }
        scopes.push((name.clone(), duration(start, end)?));
    }
    Some(GpuSample {
        stamp,
        elapsed_ms,
        scopes,
        invalid_scopes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_conversion_preserves_source_frame_and_nested_spans() {
        let stamp = Stamp {
            frame: 120,
            epoch: 3,
            ..default()
        };
        let sample = decode(
            stamp,
            &[100, 1_000_100, 200, 500_200],
            &["opaque".into()],
            2.0,
        )
        .unwrap();
        assert_eq!(sample.stamp.frame, 120);
        assert_eq!(sample.stamp.epoch, 3);
        assert_eq!(sample.elapsed_ms, 2.0);
        assert_eq!(sample.scopes[0].1, 1.0);
    }

    #[test]
    fn invalid_or_missing_timestamps_never_become_fake_durations() {
        for values in [&[0, 100][..], &[100, 0], &[100, 100], &[200, 100], &[]] {
            assert!(decode(default(), values, &[], 1.0).is_none());
        }
        assert!(decode(default(), &[100, 200], &[], f64::NAN).is_none());
        let partial = decode(default(), &[100, 200, 90, 150], &["outside".into()], 1.0).unwrap();
        assert!(partial.scopes.is_empty());
        assert_eq!(partial.invalid_scopes, 1);
        let partial = decode(default(), &[100, 200, 110, 210], &["outside".into()], 1.0).unwrap();
        assert!(partial.scopes.is_empty());
        assert_eq!(partial.invalid_scopes, 1);
        let partial = decode(default(), &[100, 200, 180, 120], &["reversed".into()], 1.0).unwrap();
        assert!(partial.scopes.is_empty());
        assert_eq!(partial.invalid_scopes, 1);
    }
}
