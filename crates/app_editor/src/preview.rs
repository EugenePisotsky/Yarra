//! Typed preview modes and isolated transient session lifecycle.

use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use engine::{WorldCatalog, WorldOrigin};
use world::{CellCoord, WorldSpaceId};

use crate::{
    derived_jobs::{DerivedArtifactStore, DerivedJobKey, DerivedJobScheduler, DerivedJobScope},
    project_store::ProjectEditorStore,
    tools::{DerivedProduct, EditorSourceDomain, EditorToolId},
    workspaces::{EditorFramePacing, EditorWorkspace, FramePacingOwner},
};

const PREVIEW_DERIVED_OWNER: EditorToolId = EditorToolId("world.preview");
const GAMEPLAY_FIXED_STEP: f32 = 1.0 / 60.0;
const MAX_GAMEPLAY_STEPS_PER_FRAME: usize = 8;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorPreviewMode {
    #[default]
    Authoring,
    Collision,
    Navigation,
    Gameplay,
}

impl EditorPreviewMode {
    pub(crate) const ALL: [Self; 4] = [
        Self::Authoring,
        Self::Collision,
        Self::Navigation,
        Self::Gameplay,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Authoring => "Authoring",
            Self::Collision => "Collision",
            Self::Navigation => "Navigation",
            Self::Gameplay => "Gameplay",
        }
    }

    pub(crate) const fn descriptor(self) -> PreviewModeDescriptor {
        match self {
            Self::Authoring => PreviewModeDescriptor {
                domains: &[
                    EditorSourceDomain::CellDescriptors,
                    EditorSourceDomain::ObjectPlacements,
                    EditorSourceDomain::ObjectDefinitions,
                ],
                isolated_simulation: false,
                full_rate: false,
            },
            Self::Collision => PreviewModeDescriptor {
                domains: &[
                    EditorSourceDomain::CellDescriptors,
                    EditorSourceDomain::ObjectPlacements,
                    EditorSourceDomain::Collision,
                ],
                isolated_simulation: false,
                full_rate: false,
            },
            Self::Navigation => PreviewModeDescriptor {
                domains: &[
                    EditorSourceDomain::CellDescriptors,
                    EditorSourceDomain::ObjectPlacements,
                    EditorSourceDomain::Navigation,
                ],
                isolated_simulation: false,
                full_rate: false,
            },
            Self::Gameplay => PreviewModeDescriptor {
                domains: &[
                    EditorSourceDomain::CellDescriptors,
                    EditorSourceDomain::ObjectPlacements,
                    EditorSourceDomain::Navigation,
                    EditorSourceDomain::Collision,
                ],
                isolated_simulation: true,
                full_rate: true,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PreviewModeDescriptor {
    pub(crate) domains: &'static [EditorSourceDomain],
    pub(crate) isolated_simulation: bool,
    pub(crate) full_rate: bool,
}

#[derive(Resource, Default)]
pub(crate) struct PreviewModeState {
    requested: EditorPreviewMode,
    active: Option<EditorPreviewMode>,
    generation: u64,
}

impl PreviewModeState {
    pub(crate) const fn requested(&self) -> EditorPreviewMode {
        self.requested
    }

    pub(crate) const fn active(&self) -> Option<EditorPreviewMode> {
        self.active
    }

    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn request(&mut self, mode: EditorPreviewMode) {
        self.requested = mode;
    }
}

#[derive(Component, Debug)]
pub(crate) struct PreviewSessionRoot {
    pub(crate) mode: EditorPreviewMode,
    pub(crate) generation: u64,
}

#[derive(Component, Debug)]
struct CollisionPreviewHost;

#[derive(Component, Debug)]
struct NavigationPreviewHost;

#[derive(Component, Debug)]
struct GameplaySimulationHost {
    accumulator: f32,
    elapsed: f32,
    actor_position: Vec3,
    ticks: u64,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct PreviewRuntimeDiagnostics {
    pub(crate) rendered_cells: usize,
    pub(crate) rendered_objects: usize,
    pub(crate) simulation_ticks: u64,
}

pub(crate) struct PreviewModesPlugin;

impl Plugin for PreviewModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PreviewModeState>()
            .init_resource::<PreviewRuntimeDiagnostics>()
            .add_systems(OnEnter(EditorWorkspace::World), activate_requested_preview)
            .add_systems(OnExit(EditorWorkspace::World), suspend_preview_session)
            .add_systems(
                Update,
                (
                    apply_preview_request,
                    request_preview_products,
                    simulate_gameplay_preview,
                )
                    .chain()
                    .run_if(world_workspace_active),
            )
            .add_systems(
                PostUpdate,
                (
                    draw_collision_preview
                        .run_if(collision_preview_active)
                        .run_if(resource_exists::<GizmoConfigStore>),
                    draw_navigation_preview
                        .run_if(navigation_preview_active)
                        .run_if(resource_exists::<GizmoConfigStore>),
                    draw_gameplay_preview
                        .run_if(gameplay_preview_active)
                        .run_if(resource_exists::<GizmoConfigStore>),
                )
                    .run_if(world_workspace_active),
            );
    }
}

fn world_workspace_active(workspace: Res<State<EditorWorkspace>>) -> bool {
    *workspace.get() == EditorWorkspace::World
}

fn apply_preview_request(
    mut commands: Commands,
    mut state: ResMut<PreviewModeState>,
    mut pacing: ResMut<EditorFramePacing>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    roots: Query<(Entity, &PreviewSessionRoot)>,
) {
    if state.active == Some(state.requested) {
        return;
    }
    for (root, session) in &roots {
        debug_assert!(session.generation <= state.generation);
        debug_assert_ne!(session.mode, EditorPreviewMode::Authoring);
        commands.entity(root).despawn();
    }
    pacing.release(FramePacingOwner::WorldGameplayPreview);
    state.generation = state.generation.wrapping_add(1).max(1);
    state.active = Some(state.requested);
    *diagnostics = PreviewRuntimeDiagnostics::default();
    let descriptor = state.requested.descriptor();
    if descriptor.full_rate {
        pacing.request(FramePacingOwner::WorldGameplayPreview);
    }
    if state.requested != EditorPreviewMode::Authoring {
        let mut root = commands.spawn((
            PreviewSessionRoot {
                mode: state.requested,
                generation: state.generation,
            },
            Name::new(format!("{} preview session", state.requested.label())),
        ));
        match state.requested {
            EditorPreviewMode::Collision => {
                root.insert(CollisionPreviewHost);
            }
            EditorPreviewMode::Navigation => {
                root.insert(NavigationPreviewHost);
            }
            EditorPreviewMode::Gameplay => {
                root.insert(GameplaySimulationHost {
                    accumulator: 0.0,
                    elapsed: 0.0,
                    actor_position: Vec3::Y,
                    ticks: 0,
                });
            }
            EditorPreviewMode::Authoring => {}
        }
    }
}

fn activate_requested_preview(
    commands: Commands,
    state: ResMut<PreviewModeState>,
    pacing: ResMut<EditorFramePacing>,
    diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    roots: Query<(Entity, &PreviewSessionRoot)>,
) {
    apply_preview_request(commands, state, pacing, diagnostics, roots);
}

fn suspend_preview_session(
    mut commands: Commands,
    mut state: ResMut<PreviewModeState>,
    mut pacing: ResMut<EditorFramePacing>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    roots: Query<(Entity, &PreviewSessionRoot)>,
) {
    for (root, session) in &roots {
        debug_assert_ne!(session.mode, EditorPreviewMode::Authoring);
        debug_assert!(session.generation <= state.generation);
        commands.entity(root).despawn();
    }
    state.active = None;
    pacing.release(FramePacingOwner::WorldGameplayPreview);
    *diagnostics = PreviewRuntimeDiagnostics::default();
}

fn request_preview_products(
    state: Res<PreviewModeState>,
    project: Option<Res<ProjectEditorStore>>,
    scheduler: Option<ResMut<DerivedJobScheduler>>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
) {
    let (Some(project), Some(mut scheduler)) = (project, scheduler) else {
        return;
    };
    let products: &[DerivedProduct] = match state.active {
        Some(EditorPreviewMode::Collision) => &[DerivedProduct::Collision],
        Some(EditorPreviewMode::Navigation) => &[DerivedProduct::Navigation],
        Some(EditorPreviewMode::Gameplay) => {
            &[DerivedProduct::Collision, DerivedProduct::Navigation]
        }
        _ => &[],
    };
    let revision = project.source_epoch().max(1);
    diagnostics.rendered_cells = project.cells().len();
    diagnostics.rendered_objects = project.objects().len();
    let requests = project.cells().iter().flat_map(|cell| {
        products.iter().map(move |product| {
            (
                DerivedJobKey {
                    product: *product,
                    scope: DerivedJobScope::Cell {
                        space: cell.space,
                        cell: cell.cell,
                    },
                },
                revision,
            )
        })
    });
    scheduler.replace_tool_requests(PREVIEW_DERIVED_OWNER, requests);
}

fn simulate_gameplay_preview(
    time: Option<Res<Time>>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    mut hosts: Query<&mut GameplaySimulationHost>,
) {
    let Some(time) = time else {
        return;
    };
    let Ok(mut host) = hosts.single_mut() else {
        return;
    };
    advance_gameplay_host(&mut host, time.delta_secs());
    diagnostics.simulation_ticks = host.ticks;
}

fn advance_gameplay_host(host: &mut GameplaySimulationHost, delta_seconds: f32) {
    host.accumulator = (host.accumulator + delta_seconds.min(0.25))
        .min(GAMEPLAY_FIXED_STEP * MAX_GAMEPLAY_STEPS_PER_FRAME as f32);
    let mut steps = 0;
    while host.accumulator >= GAMEPLAY_FIXED_STEP && steps < MAX_GAMEPLAY_STEPS_PER_FRAME {
        host.accumulator -= GAMEPLAY_FIXED_STEP;
        host.elapsed += GAMEPLAY_FIXED_STEP;
        host.ticks = host.ticks.saturating_add(1);
        let angle = host.elapsed * 0.7;
        host.actor_position = Vec3::new(angle.sin() * 4.0, 1.0, angle.cos() * 4.0);
        steps += 1;
    }
}

fn draw_collision_preview(
    hosts: Query<(), With<CollisionPreviewHost>>,
    project: Res<ProjectEditorStore>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    artifacts: Res<DerivedArtifactStore>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    mut gizmos: Gizmos,
) {
    if hosts.is_empty() {
        return;
    }
    diagnostics.rendered_cells = project.cells().len();
    diagnostics.rendered_objects = project.objects().len();
    for cell in project.cells() {
        let Some(space) = catalog.world_space(cell.space) else {
            continue;
        };
        let ready = artifacts
            .get(preview_job_key(
                DerivedProduct::Collision,
                cell.space,
                cell.cell,
            ))
            .is_some();
        draw_cell_square(
            &mut gizmos,
            cell.space,
            cell.cell,
            cell.height + 0.1,
            space.cell_size,
            origin.cell(),
            if ready {
                Color::srgba(0.18, 0.82, 1.0, 0.9)
            } else {
                Color::srgba(1.0, 0.65, 0.15, 0.75)
            },
        );
    }
    for object in project.objects() {
        let Some(space) = catalog.world_space(object.object.space) else {
            continue;
        };
        let center = source_position(
            object.object.space,
            object.object.owner_cell,
            object.object.local_translation,
            space.cell_size,
            origin.cell(),
        );
        let bounds = Vec3::from_array(object.visual_bounds.unwrap_or([1.0, 1.0, 1.0]));
        draw_bounds(
            &mut gizmos,
            center,
            bounds,
            Color::srgba(0.12, 0.82, 1.0, 0.85),
        );
    }
}

fn draw_navigation_preview(
    hosts: Query<(), With<NavigationPreviewHost>>,
    project: Res<ProjectEditorStore>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    artifacts: Res<DerivedArtifactStore>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    mut gizmos: Gizmos,
) {
    if hosts.is_empty() {
        return;
    }
    diagnostics.rendered_cells = project.cells().len();
    diagnostics.rendered_objects = project.objects().len();
    let mut centers = Vec::new();
    for cell in project.cells() {
        let Some(space) = catalog.world_space(cell.space) else {
            continue;
        };
        let center = source_position(
            cell.space,
            cell.cell,
            [
                space.cell_size * 0.5,
                cell.height + 0.18,
                space.cell_size * 0.5,
            ],
            space.cell_size,
            origin.cell(),
        );
        centers.push((cell.space, cell.cell, center));
        let ready = artifacts
            .get(preview_job_key(
                DerivedProduct::Navigation,
                cell.space,
                cell.cell,
            ))
            .is_some();
        let extent = space.cell_size * 0.18;
        let color = if ready {
            Color::srgba(0.35, 1.0, 0.42, 0.9)
        } else {
            Color::srgba(1.0, 0.68, 0.16, 0.75)
        };
        gizmos.line(center - Vec3::X * extent, center + Vec3::X * extent, color);
        gizmos.line(center - Vec3::Z * extent, center + Vec3::Z * extent, color);
    }
    for (space, cell, center) in &centers {
        for offset in [CellCoord { x: 1, z: 0 }, CellCoord { x: 0, z: 1 }] {
            let neighbor = CellCoord {
                x: cell.x.saturating_add(offset.x),
                z: cell.z.saturating_add(offset.z),
            };
            if let Some((_, _, neighbor_center)) =
                centers.iter().find(|(other_space, other_cell, _)| {
                    other_space == space && *other_cell == neighbor
                })
            {
                gizmos.line(
                    *center,
                    *neighbor_center,
                    Color::srgba(0.32, 0.95, 0.4, 0.65),
                );
            }
        }
    }
}

fn draw_gameplay_preview(
    hosts: Query<&GameplaySimulationHost>,
    mut diagnostics: ResMut<PreviewRuntimeDiagnostics>,
    mut gizmos: Gizmos,
) {
    let Ok(host) = hosts.single() else {
        return;
    };
    diagnostics.simulation_ticks = host.ticks;
    let feet = host.actor_position;
    let head = feet + Vec3::Y * 1.8;
    let color = Color::srgba(1.0, 0.35, 0.2, 0.95);
    gizmos.line(feet, head, color);
    gizmos.line(head - Vec3::X * 0.35, head + Vec3::X * 0.35, color);
    gizmos.line(feet - Vec3::X * 0.4, feet + Vec3::X * 0.4, color);
    let forward = Vec3::new(host.elapsed.cos(), 0.0, -host.elapsed.sin());
    gizmos.line(feet + Vec3::Y, feet + Vec3::Y + forward * 1.5, color);
}

fn preview_job_key(product: DerivedProduct, space: WorldSpaceId, cell: CellCoord) -> DerivedJobKey {
    DerivedJobKey {
        product,
        scope: DerivedJobScope::Cell { space, cell },
    }
}

fn source_position(
    _space: WorldSpaceId,
    cell: CellCoord,
    local: [f32; 3],
    cell_size: f32,
    origin: CellCoord,
) -> Vec3 {
    Vec3::new(
        (i64::from(cell.x) - i64::from(origin.x)) as f32 * cell_size + local[0],
        local[1],
        (i64::from(cell.z) - i64::from(origin.z)) as f32 * cell_size + local[2],
    )
}

fn draw_cell_square(
    gizmos: &mut Gizmos,
    space: WorldSpaceId,
    cell: CellCoord,
    height: f32,
    cell_size: f32,
    origin: CellCoord,
    color: Color,
) {
    let minimum = source_position(space, cell, [0.0, height, 0.0], cell_size, origin);
    let corners = [
        minimum,
        minimum + Vec3::X * cell_size,
        minimum + Vec3::new(cell_size, 0.0, cell_size),
        minimum + Vec3::Z * cell_size,
    ];
    for edge in 0..4 {
        gizmos.line(corners[edge], corners[(edge + 1) % 4], color);
    }
}

fn draw_bounds(gizmos: &mut Gizmos, base: Vec3, bounds: Vec3, color: Color) {
    let corners = std::array::from_fn::<_, 8, _>(|index| {
        base + Vec3::new(
            if index & 1 == 0 {
                -bounds.x * 0.5
            } else {
                bounds.x * 0.5
            },
            if index & 4 == 0 { 0.0 } else { bounds.y },
            if index & 2 == 0 {
                -bounds.z * 0.5
            } else {
                bounds.z * 0.5
            },
        )
    });
    for (start, end) in [
        (0, 1),
        (1, 3),
        (3, 2),
        (2, 0),
        (4, 5),
        (5, 7),
        (7, 6),
        (6, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        gizmos.line(corners[start], corners[end], color);
    }
}

pub(crate) fn authoring_preview_active(state: Res<PreviewModeState>) -> bool {
    state.active == Some(EditorPreviewMode::Authoring)
}

fn collision_preview_active(state: Res<PreviewModeState>) -> bool {
    state.active == Some(EditorPreviewMode::Collision)
}

fn navigation_preview_active(state: Res<PreviewModeState>) -> bool {
    state.active == Some(EditorPreviewMode::Navigation)
}

fn gameplay_preview_active(state: Res<PreviewModeState>) -> bool {
    state.active == Some(EditorPreviewMode::Gameplay)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::EditorWorkspacesPlugin;

    #[test]
    fn gameplay_preview_has_an_isolated_full_rate_session_lifecycle() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins((EditorWorkspacesPlugin, PreviewModesPlugin));
        app.update();
        app.world_mut()
            .resource_mut::<PreviewModeState>()
            .request(EditorPreviewMode::Gameplay);
        app.update();

        let (active, generation) = {
            let state = app.world().resource::<PreviewModeState>();
            (state.active(), state.generation())
        };
        assert_eq!(active, Some(EditorPreviewMode::Gameplay));
        assert!(
            app.world()
                .resource::<EditorFramePacing>()
                .full_rate_preview()
        );
        let root = app
            .world_mut()
            .query::<&PreviewSessionRoot>()
            .single(app.world())
            .unwrap();
        assert_eq!(root.mode, EditorPreviewMode::Gameplay);
        assert_eq!(root.generation, generation);

        app.world_mut()
            .resource_mut::<NextState<EditorWorkspace>>()
            .set(EditorWorkspace::Animation);
        app.update();
        assert_eq!(app.world().resource::<PreviewModeState>().active(), None);
    }

    #[test]
    fn gameplay_host_uses_a_capped_fixed_step() {
        let mut host = GameplaySimulationHost {
            accumulator: 0.0,
            elapsed: 0.0,
            actor_position: Vec3::Y,
            ticks: 0,
        };
        advance_gameplay_host(&mut host, 1.0);
        assert_eq!(host.ticks, MAX_GAMEPLAY_STEPS_PER_FRAME as u64);
        assert_eq!(host.elapsed, GAMEPLAY_FIXED_STEP * host.ticks as f32);
        assert_ne!(host.actor_position, Vec3::Y);
    }
}
