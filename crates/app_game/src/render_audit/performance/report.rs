//! Shared live/capture formatting, duration statistics and report/CSV export.
use super::{
    capture::CaptureSession,
    timing::{self, Kind},
};
use crate::render_audit::Control;
use std::time::{SystemTime, UNIX_EPOCH};

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
    Control::PageGizmos,
    Control::TerrainMacro,
];

#[derive(Debug, Default)]
pub(super) struct Stats {
    pub(super) mean: f64,
    pub(super) median: f64,
    pub(super) p95: f64,
    pub(super) p99: f64,
}
pub(super) fn stats(values: impl IntoIterator<Item = f64>) -> Stats {
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
pub(super) fn comparison(state: &CaptureSession) -> String {
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
        if a.settings.canopy != b.settings.canopy {
            differences += 1;
            out += "Canopy look changed (full values are included in exported settings)\n";
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
pub(super) fn timing_summary(gpu: &[timing::GpuSample], cpu: &[timing::CpuSample]) -> String {
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
pub(super) fn timing_details(history: &timing::History, stamp: timing::Stamp) -> String {
    if !history.enabled {
        return history.status.clone();
    }
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
    if !stamp.detailed && !history.gpu_disabled {
        out += "Enable GPU pass timings above for a breakdown (adds probe overhead).\n";
    } else if stamp.detailed {
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

pub(super) fn export(state: &CaptureSession) -> std::io::Result<std::path::PathBuf> {
    if state.slots.iter().all(Option::is_none) {
        return Err(std::io::Error::other("Capture A or B first."));
    }
    let root = std::env::var_os("YARRA_PERFORMANCE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "tmp/performance".into());
    export_to(state, &root)
}
pub(super) fn export_to(
    state: &CaptureSession,
    root: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
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
mod tests;
