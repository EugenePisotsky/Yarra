//! Read-only world, streaming, source and publication diagnostics.
use crate::{
    derived_jobs::{DerivedArtifactStore, DerivedJobScheduler},
    domain_editing::DenseDomainWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet},
    journal::EditorJournalStatus,
    navigation::ProjectNavigationStore,
    overview::{OverviewProductKind, OverviewState},
    preview::{EditorPreviewMode, PreviewModeState, PreviewRuntimeDiagnostics},
    project_store::{ProjectEditorStore, ProjectQueryWindow},
    publication::RuntimePublicationState,
    tools::EditorToolRegistry,
    workspaces::EditorWorkspace,
};
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};
use bevy_egui::egui;
use engine::{StreamingStats, WorldCatalog, WorldOrigin, WorldViewpoint};

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_world_diagnostics(
    ui: &mut egui::Ui,
    diagnostics: &DiagnosticsStore,
    catalog: &WorldCatalog,
    viewpoint: &WorldViewpoint,
    origin: &WorldOrigin,
    stats: &StreamingStats,
    derived_jobs: &DerivedJobScheduler,
    derived_artifacts: &DerivedArtifactStore,
    dense_domains: &DenseDomainWorkingSets,
    journal: &EditorJournalStatus,
    navigation: &ProjectNavigationStore,
    overview: &OverviewState,
    preview: &PreviewModeState,
    preview_runtime: &PreviewRuntimeDiagnostics,
    publication: &RuntimePublicationState,
    project: &ProjectEditorStore,
    objects: &EditorObjectWorkingSet,
    history: &EditorHistory,
    tools: &EditorToolRegistry,
) {
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|diagnostic| diagnostic.smoothed())
        .unwrap_or_default();
    ui.monospace(format!("{fps:.1} FPS · 30 FPS authoring target"));
    ui.label(&stats.status);
    if !catalog.generation_id().is_empty() {
        ui.small(format!("Runtime generation {}", catalog.generation_id()));
    }
    ui.small(publication.status());
    if let Some(generation) = publication.published_generation() {
        ui.small(format!("Last publication adopted: {generation}"));
    }

    ui.separator();
    ui.heading("Logical viewpoint");
    if let Some(position) = viewpoint.position() {
        ui.monospace(format!(
            "space {} · cell {}, {} · local {:.2}, {:.2}, {:.2}",
            position.space.0,
            position.cell.x,
            position.cell.z,
            position.local[0],
            position.local[1],
            position.local[2]
        ));
    } else {
        ui.weak("Waiting for runtime manifest…");
    }
    ui.monospace(format!("origin {}, {}", origin.cell().x, origin.cell().z));

    ui.separator();
    ui.heading("Streaming");
    egui::Grid::new("streaming_diagnostics")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            diagnostic_row(ui, "Demanded", stats.demanded);
            diagnostic_row(ui, "Loading", stats.loading);
            diagnostic_row(ui, "Prepared", stats.prepared);
            diagnostic_row(ui, "Resident", stats.resident);
            diagnostic_row(ui, "Cooling", stats.cooling);
            diagnostic_row(ui, "Failed", stats.failed);
            diagnostic_row(ui, "Page entities", stats.owned_entities);
            diagnostic_row(ui, "Vegetation V2 pages", stats.vegetation_pages);
            ui.label("Decoded");
            ui.monospace(format_bytes(stats.decoded_bytes));
            ui.end_row();
            ui.label("GPU estimate");
            ui.monospace(format_bytes(stats.gpu_bytes_estimate));
            ui.end_row();
        });
    ui.small("Editor profile: 64 MiB decoded / 256 MiB estimated GPU");

    ui.separator();
    ui.heading("Authoring source");
    ui.label(project.status());
    if let Some(active_tool) = tools.active(EditorWorkspace::World) {
        ui.small(format!(
            "{} · {} source domain(s) · {:?}",
            active_tool.label,
            active_tool.source_domains.len(),
            active_tool.pinning
        ));
    }
    ui.small(format!(
        "{} coverage cell(s) · {} unsaved · {} retained",
        dense_domains.environment_record_count(),
        dense_domains.dirty_count(),
        format_bytes(dense_domains.retained_bytes() as u64),
    ));
    ui.small(format!(
        "{} · recovered this session {}",
        journal.message(),
        journal.recovered_entries()
    ));
    ui.small(format!(
        "Derived: {} pending / {} running · {} accepted / {} failed · {} cancelled / {} stale / {} capacity retries",
        derived_jobs.pending_count(),
        derived_jobs.running_count(),
        derived_artifacts.accepted_count(),
        derived_artifacts.failed_jobs(),
        derived_jobs.cancelled_count(),
        derived_jobs.stale_results(),
        derived_jobs.capacity_rejections()
    ));
    ui.small(format!(
        "{} · {} stale navigation result(s)",
        navigation.status(),
        navigation.stale_results()
    ));
    if let Some(error) = project.write_error() {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!("Read-only authoring session: {error}"),
        );
    }
    if let Some(manifest) = project.manifest() {
        ui.small(format!(
            "Schema {} · {} world spaces · default {}",
            manifest.schema_version,
            manifest.world_spaces.len(),
            manifest.default_world_space.0
        ));
    }
    egui::Grid::new("source_cache_diagnostics")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            diagnostic_row(ui, "Cached cells", project.cells().len());
            diagnostic_row(ui, "Cached objects", project.objects().len());
            ui.label("Query");
            ui.monospace(if project.query_in_flight() {
                "in flight"
            } else {
                "idle"
            });
            ui.end_row();
            ui.label("Save");
            ui.monospace(if project.save_in_flight() {
                "in flight"
            } else {
                "idle"
            });
            ui.end_row();
            ui.label("Highest revision");
            ui.monospace(
                project
                    .highest_source_revision()
                    .map_or_else(|| "—".into(), |revision| revision.to_string()),
            );
            ui.end_row();
            ui.label("Completed queries");
            ui.monospace(project.completed_queries().to_string());
            ui.end_row();
            ui.label("Stale results");
            ui.monospace(project.stale_results().to_string());
            ui.end_row();
        });
    if let Some(window) = project.desired_window() {
        ui.small(format_source_window("Desired", window));
    }
    if let Some(window) = project.loaded_window() {
        ui.small(format_source_window("Loaded", window));
    }
    if project.cells_truncated() || project.objects_truncated() {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Source result reached its hard cache limit; authoring detail is incomplete.",
        );
    }

    ui.separator();
    ui.heading("Overview and preview");
    let ready_products = overview
        .products()
        .iter()
        .filter(|product| product.state == crate::overview::OverviewProductState::Ready)
        .count();
    ui.small(format!(
        "{:?} · {} coarse tiles · {ready_products}/{} products ready",
        overview.mode(),
        overview.tiles().len(),
        overview.products().len()
    ));
    ui.small(
        OverviewProductKind::ALL
            .into_iter()
            .map(|kind| {
                let ready = overview
                    .products()
                    .iter()
                    .filter(|product| {
                        product.kind == kind
                            && product.state == crate::overview::OverviewProductState::Ready
                    })
                    .count();
                format!("{} {ready}/{}", kind.label(), overview.tiles().len())
            })
            .collect::<Vec<_>>()
            .join(" · "),
    );
    let preview_descriptor = preview.requested().descriptor();
    ui.small(format!(
        "{} preview · generation {} · {} domain(s) · isolated simulation {}",
        preview.requested().label(),
        preview.generation(),
        preview_descriptor.domains.len(),
        preview_descriptor.isolated_simulation
    ));
    if preview.requested() != EditorPreviewMode::Authoring {
        ui.small(format!(
            "Preview host: {} cell(s), {} object(s), {} fixed simulation tick(s)",
            preview_runtime.rendered_cells,
            preview_runtime.rendered_objects,
            preview_runtime.simulation_ticks,
        ));
    }

    ui.separator();
    ui.heading("Local commands");
    ui.small(format!(
        "{} dirty object(s) · {} undo / {} redo · {} retained · {} checkpointed",
        objects.dirty_count(),
        history.undo_len(),
        history.redo_len(),
        format_bytes(history.retained_bytes() as u64),
        history.checkpointed_commands()
    ));
}

fn diagnostic_row(ui: &mut egui::Ui, label: &str, value: usize) {
    ui.label(label);
    ui.monospace(value.to_string());
    ui.end_row();
}

fn format_source_window(label: &str, window: ProjectQueryWindow) -> String {
    format!(
        "{label}: space {} · ({}, {})–({}, {})",
        window.space.0, window.minimum.x, window.minimum.z, window.maximum.x, window.maximum.z
    )
}

fn format_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    format!("{:.1} MiB", bytes as f64 / MIB)
}
