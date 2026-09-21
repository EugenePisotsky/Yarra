//! Infrequent, self-contained records that can be correlated with batched Metal HUD output.
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::{
    diagnostic::FrameCount,
    prelude::*,
    window::{Monitor, OnMonitor, PrimaryWindow, WindowMode},
};
use engine::WorldViewCamera;
use terrain_render::{TerrainCacheStats, TerrainMacroVariation};
use vegetation_render::{
    VegetationDebugSettings, VegetationDiagnostics, VegetationDiagnosticsSnapshot,
};

use super::{AuditAssets, AuditRenderPath, RuntimeSettings};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PowerState {
    pub(super) thermal: &'static str,
    pub(super) low_power: &'static str,
}

#[cfg(target_vendor = "apple")]
pub(super) fn power_state() -> PowerState {
    use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};
    let process = NSProcessInfo::processInfo();
    PowerState {
        thermal: match process.thermalState() {
            NSProcessInfoThermalState::Nominal => "nominal",
            NSProcessInfoThermalState::Fair => "fair",
            NSProcessInfoThermalState::Serious => "serious",
            NSProcessInfoThermalState::Critical => "critical",
            _ => "unknown",
        },
        low_power: if process.isLowPowerModeEnabled() {
            "on"
        } else {
            "off"
        },
    }
}

#[cfg(not(target_vendor = "apple"))]
pub(super) fn power_state() -> PowerState {
    PowerState {
        thermal: "unavailable",
        low_power: "unavailable",
    }
}

#[derive(Default)]
pub(super) struct LogState {
    last_poll_s: Option<f64>,
    last_emit: Option<(f64, u32)>,
    last_power: Option<PowerState>,
    changed_at_s: f64,
    sequence: u64,
}

impl LogState {
    fn should_poll(&self, now: f64, changed: bool) -> bool {
        changed || self.last_poll_s.is_none_or(|last| now - last >= 1.0)
    }

    fn event(&mut self, now: f64, changed: bool, power: PowerState) -> Option<&'static str> {
        self.last_poll_s = Some(now);
        let power_changed = self.last_power.is_some_and(|old| old != power);
        self.last_power = Some(power);
        if self.last_emit.is_none() {
            self.changed_at_s = now;
            Some("startup")
        } else if changed {
            self.changed_at_s = now;
            Some("change")
        } else if power_changed {
            Some("power")
        } else if self.last_emit.is_some_and(|(last, _)| now - last >= 5.0) {
            Some("sample")
        } else {
            None
        }
    }
}

fn gpu_readback_fields(snapshot: VegetationDiagnosticsSnapshot) -> String {
    if snapshot.gpu_samples == 0 {
        return "gpu_readback=unavailable".into();
    }
    // A sampled snapshot can lag the switch and represents retained buffers in frozen mode.
    format!(
        "gpu_readback=sampled gpu_samples={} sampled_scheduled_items={} sampled_candidate_lanes={} sampled_instances={} sampled_indices={} sampled_capacity_drops={:?} sampled_bins={:?}",
        snapshot.gpu_samples,
        snapshot.scheduled_work_items,
        snapshot.dispatched_candidate_lanes,
        snapshot
            .emitted_instances
            .iter()
            .map(|&n| u64::from(n))
            .sum::<u64>(),
        snapshot.submitted_indices,
        snapshot.capacity_dropped_instances,
        snapshot.emitted_instances,
    )
}

#[allow(clippy::too_many_arguments)] // Independent, read-only Bevy diagnostic resources.
pub(super) fn log_status(
    settings: Res<RuntimeSettings>,
    grass: Res<VegetationDebugSettings>,
    time: Res<Time<Real>>,
    frame: Res<FrameCount>,
    window: Single<(&Window, Option<&OnMonitor>), With<PrimaryWindow>>,
    monitors: Query<&Monitor>,
    camera: Single<
        (
            &GlobalTransform,
            &Msaa,
            Option<&engine::MsaaColorStorePolicy>,
        ),
        With<WorldViewCamera>,
    >,
    assets: Res<AuditAssets>,
    images: Res<Assets<Image>>,
    meshes: Res<Assets<Mesh>>,
    vegetation: Res<VegetationDiagnostics>,
    terrain_modes: (Res<TerrainMacroVariation>, Res<engine::TerrainLodPreview>),
    terrain_cache: Res<TerrainCacheStats>,
    terrain_prepared: Res<terrain_render::TerrainPreparedStats>,
    entities: Query<Entity>,
    mut state: Local<LogState>,
) {
    let (terrain_macro, terrain_lod) = terrain_modes;
    let terrain_lod = terrain_lod.enabled;
    let (window, on_monitor) = window.into_inner();
    // Real time is unaffected by game time clamping, pausing, or profiling's control lock.
    let now = time.elapsed_secs_f64();
    let changed = settings.is_changed() || grass.is_changed() || terrain_macro.is_changed();
    if !state.should_poll(now, changed) {
        return;
    }
    let power = power_state();
    let Some(event) = state.event(now, changed, power) else {
        return;
    };
    let (window_s, app_fps) = match state.last_emit {
        Some((last, last_frame)) if now > last => {
            let duration = now - last;
            (
                duration,
                format!(
                    "{:.2}",
                    f64::from(frame.0.wrapping_sub(last_frame)) / duration
                ),
            )
        }
        _ => (0.0, "unavailable".into()),
    };
    state.last_emit = Some((now, frame.0));
    state.sequence += 1;
    let unix_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| "unavailable".into(),
        |duration| duration.as_millis().to_string(),
    );
    let size = match settings.render_path {
        AuditRenderPath::Composite => images
            .get(&assets.target)
            .map(Image::size)
            .unwrap_or(UVec2::ZERO),
        AuditRenderPath::Direct => window.physical_size(),
    };
    let position = camera.0.translation();
    let rotation = camera.0.rotation();
    let msaa_samples = camera.1.samples();
    let msaa_store_policy = camera.2.copied().unwrap_or_default();
    let requested_msaa_samples = settings.msaa.samples();
    let snapshot = vegetation.snapshot();
    let gpu = gpu_readback_fields(snapshot);
    let ground_shader = format!("{:?}", settings.terrain_shading());
    let macro_state = terrain_macro.label();
    let render_path = settings.render_path.label();
    let ui = if settings.show_ui { "on" } else { "off" };
    let focused = window.focused;
    let present_mode = window.present_mode;
    let window_mode = match window.mode {
        WindowMode::Windowed => "windowed",
        WindowMode::BorderlessFullscreen(_) => "fullscreen",
        WindowMode::Fullscreen(_, _) => "exclusive_fullscreen",
    };
    let monitor = on_monitor.and_then(|m| monitors.get(m.0).ok());
    let monitor_px = monitor.map_or_else(
        || "unknown".into(),
        |m| format!("{}x{}", m.physical_width, m.physical_height),
    );
    let monitor_hz = monitor.and_then(|m| m.refresh_rate_millihertz).map_or_else(
        || "unknown".into(),
        |hz| format!("{:.3}", hz as f64 / 1000.0),
    );
    let display_fields = format!(
        "window_mode={window_mode} monitor_px={monitor_px} monitor_hz={monitor_hz} window_logical={}x{} scale_factor={}",
        window.width(),
        window.height(),
        window.scale_factor()
    );
    let cache = terrain_cache.snapshot();
    let prepared = terrain_prepared.snapshot();
    let terrain_cache_fields = format!(
        "terrain_cache={} terrain_cache_tables={} terrain_cache_ready={} terrain_cache_bytes={} terrain_cache_builds={} terrain_cache_reused_frames={} terrain_prepared={} terrain_prepared_pages={} terrain_prepared_active={} terrain_prepared_albedo_active={} terrain_prepared_bytes={} terrain_prepared_builds={} terrain_prepared_albedo_bytes={} terrain_prepared_albedo_images={} terrain_prepared_astc8x8_images={}",
        cache.enabled,
        cache.tables,
        cache.ready,
        cache.bytes,
        cache.builds,
        cache.reused_frames,
        prepared.enabled,
        prepared.pages,
        prepared.active,
        prepared.albedo_active,
        prepared.bytes,
        prepared.builds,
        prepared.albedo_bytes,
        prepared.albedo_images,
        prepared.astc_8x8_images
    );
    let blade_preparation = format!(
        "blade_preparation={} blade_preparation_bytes={} blade_preparation_dispatches={} blade_preparation_reuses={} sampled_prepared_blades={} sampled_preparation_fallback_blades={}",
        snapshot.blade_preparation_enabled,
        snapshot.blade_preparation_bytes,
        snapshot.blade_preparation_dispatches,
        snapshot.blade_preparation_reuses,
        snapshot.prepared_blades,
        snapshot.preparation_fallback_blades
    );
    let early_rejection = grass.early_rejection;
    let candidate_cache = format!(
        "candidate_cache={} candidate_cache_bytes={} candidate_cache_builds={} candidate_cache_ready={} candidate_cache_planned={}",
        snapshot.candidate_cache_enabled,
        snapshot.candidate_cache_bytes,
        snapshot.candidate_cache_builds,
        snapshot.candidate_cache_ready_items,
        snapshot.candidate_cache_planned_items
    );
    let terrain_near = if settings.terrain_near_disabled {
        "off"
    } else {
        "on"
    };
    let generation_dispatches = snapshot.generation_dispatches;
    let generation_reuses = snapshot.generation_reuses;
    warn!(
        "RENDER_AUDIT v=1 event={event} seq={} unix_ms={unix_ms} elapsed_s={now:.3} main_frame={} app_fps_window={app_fps} window_s={window_s:.3} since_change_s={:.3} thermal={} low_power={} scene={:?} grass={} unlit={} ground_shader={ground_shader} terrain_lod={terrain_lod} terrain_near={terrain_near} terrain_macro={macro_state} shadows={} prepass={} scale={} msaa_samples={msaa_samples} requested_msaa_samples={requested_msaa_samples} msaa_store_policy={msaa_store_policy:?} counters={} wind={} locked={} render_path={render_path} ui={ui} render_px={}x{} surface_px={}x{} {display_fields} focused={focused} present_mode={present_mode:?} camera_pos={:.3},{:.3},{:.3} camera_rot={:.4},{:.4},{:.4},{:.4} density={:?} lighting={:?} far_width_compensation={} entities={} mesh_assets={} image_assets={} source_revision={} source_pages={} source_work_items={} source_repacks={} source_reallocs={} last_source_upload_bytes={} source_capacity_bytes={} instance_capacity={} instance_capacity_bytes={} generation_dispatches={generation_dispatches} generation_reuses={generation_reuses} early_rejection={early_rejection} {candidate_cache} {terrain_cache_fields} {blade_preparation} {gpu} os={} debug_assertions={}",
        state.sequence,
        frame.0,
        now - state.changed_at_s,
        power.thermal,
        power.low_power,
        settings.scene,
        grass.profile_mode.label(),
        settings.unlit,
        ["gaussian", "hardware2x2", "off"][settings.shadows as usize],
        settings.prepass,
        settings.scale(),
        grass.gpu_counters_enabled,
        settings.wind,
        settings.controls_locked,
        size.x,
        size.y,
        window.physical_width(),
        window.physical_height(),
        position.x,
        position.y,
        position.z,
        rotation.x,
        rotation.y,
        rotation.z,
        rotation.w,
        grass.density_mode,
        grass.lighting_mode,
        grass.far_width_compensation,
        entities.iter().len(),
        meshes.len(),
        images.len(),
        snapshot.scene_revision,
        snapshot.source_pages,
        snapshot.source_work_items,
        snapshot.source_repacks,
        snapshot.source_buffer_reallocations,
        snapshot.source_uploaded_bytes,
        snapshot.source_buffer_capacity_bytes,
        snapshot.procedural_instance_capacity,
        snapshot.procedural_instance_bytes,
        std::env::consts::OS,
        cfg!(debug_assertions),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_vendor = "apple")]
    fn native_power_state_can_be_read() {
        let power = power_state();
        assert!(matches!(
            power.thermal,
            "nominal" | "fair" | "serious" | "critical" | "unknown"
        ));
        assert!(matches!(power.low_power, "on" | "off"));
    }

    #[test]
    fn cadence_logs_changes_and_power_transitions_without_per_frame_output() {
        let nominal = PowerState {
            thermal: "nominal",
            low_power: "off",
        };
        let serious = PowerState {
            thermal: "serious",
            low_power: "off",
        };
        let mut state = LogState::default();
        assert!(state.should_poll(0.0, false));
        assert_eq!(state.event(0.0, false, nominal), Some("startup"));
        state.last_emit = Some((0.0, 0));
        assert!(!state.should_poll(0.1, false));
        assert!(state.should_poll(0.1, true));
        assert_eq!(state.event(0.1, true, nominal), Some("change"));
        state.last_emit = Some((0.1, 6));
        assert_eq!(state.event(1.1, false, serious), Some("power"));
        state.last_emit = Some((1.1, 66));
        assert_eq!(state.event(2.1, false, serious), None);
        assert_eq!(state.event(6.2, false, serious), Some("sample"));
        assert_eq!(
            state.changed_at_s, 0.1,
            "sampling must not reset the setting age"
        );
    }

    #[test]
    fn missing_gpu_readback_is_not_reported_as_zero_work() {
        assert_eq!(gpu_readback_fields(default()), "gpu_readback=unavailable");
        let fields = gpu_readback_fields(VegetationDiagnosticsSnapshot {
            gpu_samples: 1,
            emitted_instances: [1, 2, 3, 4],
            ..default()
        });
        assert!(fields.contains("gpu_readback=sampled"));
        assert!(fields.contains("sampled_instances=10"));
    }
}
