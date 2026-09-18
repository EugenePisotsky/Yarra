//! Declarative contracts for bounded editor tools.
//!
//! A tool is not identified by a panel or an ECS entity. Its descriptor states which source data
//! it may demand, what must remain pinned, which command boundary it produces, what it draws over
//! the cooked world, and which derived products become stale. This keeps future terrain,
//! vegetation, and navigation tools on the same bounded architecture as object editing.

#![allow(
    dead_code,
    reason = "the contract intentionally reserves domains and policies for the next registered tools"
)]

use std::collections::HashMap;

use bevy::prelude::*;

use crate::workspaces::EditorWorkspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EditorToolId(pub(crate) &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorSourceDomain {
    CellDescriptors,
    ObjectPlacements,
    ObjectDefinitions,
    EnvironmentCoverage,
    RoadRecords,
    VegetationCatalog,
    VegetationFields,
    Navigation,
    Collision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpatialQueryPolicy {
    ViewpointWindow {
        radius_cells: u32,
        maximum_records: usize,
    },
    ExplicitSelection,
    OverviewTiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PinningPolicy {
    SelectedDirtyAndActiveCommand,
    ActivePatchAndDirtyCells,
    CatalogDraftAndResidentFields,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorCommandKind {
    CreatePlacement,
    TransformPlacement,
    DeletePlacement,
    PaintEnvironment,
    EditRoad,
    EditVegetationProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorPreviewOverlay {
    SourceObjectProxy,
    SelectionBounds,
    TransformGizmo,
    CellPatch,
    RoadCurve,
    ProceduralVegetation,
    VegetationGroups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DerivedProduct {
    CookedObjectPage,
    EnvironmentCoverage,
    Navigation,
    Collision,
    Overview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadingPolicy {
    KeepCameraResponsiveWithProxies,
    DisableGestureUntilLoaded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictPolicy {
    PreserveLocalCommandForResolution,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancellationPolicy {
    CancelObsoleteQueriesAndDerivedJobs,
    FinishAtomicWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailurePolicy {
    KeepToolAndCameraUsable,
    DisableTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditorToolFailurePolicy {
    pub(crate) loading: LoadingPolicy,
    pub(crate) conflict: ConflictPolicy,
    pub(crate) cancellation: CancellationPolicy,
    pub(crate) failure: FailurePolicy,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EditorToolDescriptor {
    pub(crate) id: EditorToolId,
    pub(crate) label: &'static str,
    pub(crate) workspace: EditorWorkspace,
    pub(crate) source_domains: &'static [EditorSourceDomain],
    pub(crate) spatial_query: SpatialQueryPolicy,
    pub(crate) pinning: PinningPolicy,
    pub(crate) commands: &'static [EditorCommandKind],
    pub(crate) overlays: &'static [EditorPreviewOverlay],
    pub(crate) invalidates: &'static [DerivedProduct],
    pub(crate) failure_policy: EditorToolFailurePolicy,
}

impl EditorToolDescriptor {
    pub(crate) fn requires(self, domain: EditorSourceDomain) -> bool {
        self.source_domains.contains(&domain)
    }
}

#[derive(Resource, Default)]
pub(crate) struct EditorToolRegistry {
    tools: HashMap<EditorToolId, EditorToolDescriptor>,
    active: HashMap<EditorWorkspace, EditorToolId>,
}

impl EditorToolRegistry {
    pub(crate) fn register(&mut self, descriptor: EditorToolDescriptor, active_by_default: bool) {
        let previous = self.tools.insert(descriptor.id, descriptor);
        assert!(
            previous.is_none(),
            "duplicate editor tool {}",
            descriptor.id.0
        );
        if active_by_default {
            let previous = self.active.insert(descriptor.workspace, descriptor.id);
            assert!(
                previous.is_none(),
                "workspace {} already has a default tool",
                descriptor.workspace.label()
            );
        }
    }

    pub(crate) fn active(&self, workspace: EditorWorkspace) -> Option<EditorToolDescriptor> {
        self.active
            .get(&workspace)
            .and_then(|id| self.tools.get(id))
            .copied()
    }

    pub(crate) fn set_active(&mut self, workspace: EditorWorkspace, tool: EditorToolId) -> bool {
        if self
            .tools
            .get(&tool)
            .is_none_or(|descriptor| descriptor.workspace != workspace)
        {
            return false;
        }
        self.active.insert(workspace, tool) != Some(tool)
    }

    pub(crate) fn tools_for(
        &self,
        workspace: EditorWorkspace,
    ) -> impl Iterator<Item = EditorToolDescriptor> + '_ {
        self.tools
            .values()
            .filter(move |descriptor| descriptor.workspace == workspace)
            .copied()
    }

    pub(crate) fn active_requires(
        &self,
        workspace: EditorWorkspace,
        domain: EditorSourceDomain,
    ) -> bool {
        self.active(workspace)
            .is_some_and(|descriptor| descriptor.requires(domain))
    }
}

pub(crate) struct EditorToolsPlugin;

impl Plugin for EditorToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EditorToolRegistry>();
    }
}

pub(crate) const OBJECT_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.objects"),
    label: "Objects",
    workspace: EditorWorkspace::World,
    source_domains: &[
        EditorSourceDomain::CellDescriptors,
        EditorSourceDomain::ObjectPlacements,
        EditorSourceDomain::ObjectDefinitions,
    ],
    spatial_query: SpatialQueryPolicy::ViewpointWindow {
        radius_cells: 2,
        maximum_records: 2_048,
    },
    pinning: PinningPolicy::SelectedDirtyAndActiveCommand,
    commands: &[
        EditorCommandKind::CreatePlacement,
        EditorCommandKind::TransformPlacement,
        EditorCommandKind::DeletePlacement,
    ],
    overlays: &[
        EditorPreviewOverlay::SourceObjectProxy,
        EditorPreviewOverlay::SelectionBounds,
        EditorPreviewOverlay::TransformGizmo,
    ],
    invalidates: &[
        DerivedProduct::CookedObjectPage,
        DerivedProduct::Navigation,
        DerivedProduct::Collision,
        DerivedProduct::Overview,
    ],
    failure_policy: EditorToolFailurePolicy {
        loading: LoadingPolicy::KeepCameraResponsiveWithProxies,
        conflict: ConflictPolicy::PreserveLocalCommandForResolution,
        cancellation: CancellationPolicy::CancelObsoleteQueriesAndDerivedJobs,
        failure: FailurePolicy::KeepToolAndCameraUsable,
    },
};

pub(crate) const ROAD_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.roads"),
    label: "Roads",
    workspace: EditorWorkspace::World,
    source_domains: &[
        EditorSourceDomain::CellDescriptors,
        EditorSourceDomain::RoadRecords,
    ],
    spatial_query: SpatialQueryPolicy::ViewpointWindow {
        radius_cells: 2,
        maximum_records: 1024,
    },
    pinning: PinningPolicy::SelectedDirtyAndActiveCommand,
    commands: &[EditorCommandKind::EditRoad],
    overlays: &[EditorPreviewOverlay::RoadCurve],
    invalidates: &[
        DerivedProduct::EnvironmentCoverage,
        DerivedProduct::Navigation,
        DerivedProduct::Overview,
    ],
    failure_policy: OBJECT_TOOL.failure_policy,
};

pub(crate) const ENVIRONMENT_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.environment"),
    label: "Environment",
    workspace: EditorWorkspace::World,
    source_domains: &[
        EditorSourceDomain::CellDescriptors,
        EditorSourceDomain::EnvironmentCoverage,
    ],
    spatial_query: SpatialQueryPolicy::ViewpointWindow {
        radius_cells: 2,
        maximum_records: 25,
    },
    pinning: PinningPolicy::ActivePatchAndDirtyCells,
    commands: &[EditorCommandKind::PaintEnvironment],
    overlays: &[EditorPreviewOverlay::CellPatch],
    invalidates: &[
        DerivedProduct::EnvironmentCoverage,
        DerivedProduct::Navigation,
        DerivedProduct::Collision,
        DerivedProduct::Overview,
    ],
    failure_policy: EditorToolFailurePolicy {
        loading: LoadingPolicy::DisableGestureUntilLoaded,
        conflict: ConflictPolicy::PreserveLocalCommandForResolution,
        cancellation: CancellationPolicy::CancelObsoleteQueriesAndDerivedJobs,
        failure: FailurePolicy::KeepToolAndCameraUsable,
    },
};

pub(crate) const VEGETATION_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.vegetation"),
    label: "Vegetation",
    workspace: EditorWorkspace::World,
    source_domains: &[
        EditorSourceDomain::CellDescriptors,
        EditorSourceDomain::VegetationCatalog,
        EditorSourceDomain::VegetationFields,
    ],
    spatial_query: SpatialQueryPolicy::ViewpointWindow {
        radius_cells: 3,
        maximum_records: 100,
    },
    pinning: PinningPolicy::CatalogDraftAndResidentFields,
    commands: &[EditorCommandKind::EditVegetationProfile],
    overlays: &[
        EditorPreviewOverlay::ProceduralVegetation,
        EditorPreviewOverlay::VegetationGroups,
    ],
    // The global catalog now persists through the project save coordinator and is consumed by the
    // runtime cook. Spatial field painting will declare its concrete bounded invalidations when
    // that source command lands; claiming page products here would be premature.
    invalidates: &[],
    failure_policy: EditorToolFailurePolicy {
        loading: LoadingPolicy::KeepCameraResponsiveWithProxies,
        conflict: ConflictPolicy::PreserveLocalCommandForResolution,
        cancellation: CancellationPolicy::CancelObsoleteQueriesAndDerivedJobs,
        failure: FailurePolicy::KeepToolAndCameraUsable,
    },
};

pub(crate) fn object_tool_active(registry: Res<EditorToolRegistry>) -> bool {
    registry
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == OBJECT_TOOL.id)
}

pub(crate) fn vegetation_tool_active(registry: Res<EditorToolRegistry>) -> bool {
    registry
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == VEGETATION_TOOL.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_tool_declares_the_complete_bounded_tool_contract() {
        assert!(OBJECT_TOOL.requires(EditorSourceDomain::ObjectPlacements));
        assert_eq!(
            OBJECT_TOOL.spatial_query,
            SpatialQueryPolicy::ViewpointWindow {
                radius_cells: 2,
                maximum_records: 2_048,
            }
        );
        assert_eq!(
            OBJECT_TOOL.pinning,
            PinningPolicy::SelectedDirtyAndActiveCommand
        );
        assert!(
            OBJECT_TOOL
                .commands
                .contains(&EditorCommandKind::TransformPlacement)
        );
        assert!(
            OBJECT_TOOL
                .overlays
                .contains(&EditorPreviewOverlay::TransformGizmo)
        );
        assert!(OBJECT_TOOL.invalidates.contains(&DerivedProduct::Overview));
        assert_eq!(
            OBJECT_TOOL.failure_policy.conflict,
            ConflictPolicy::PreserveLocalCommandForResolution
        );
    }

    #[test]
    fn registry_resolves_active_tool_domains_per_workspace() {
        let mut registry = EditorToolRegistry::default();
        registry.register(OBJECT_TOOL, true);
        registry.register(ENVIRONMENT_TOOL, false);
        registry.register(VEGETATION_TOOL, false);
        assert!(registry.active_requires(
            EditorWorkspace::World,
            EditorSourceDomain::ObjectDefinitions
        ));
        assert!(!registry.active_requires(
            EditorWorkspace::Animation,
            EditorSourceDomain::ObjectDefinitions
        ));
        assert!(registry.set_active(EditorWorkspace::World, ENVIRONMENT_TOOL.id));
        assert!(registry.active_requires(
            EditorWorkspace::World,
            EditorSourceDomain::EnvironmentCoverage
        ));
        assert!(
            !registry.active_requires(EditorWorkspace::World, EditorSourceDomain::ObjectPlacements)
        );
        assert_eq!(registry.tools_for(EditorWorkspace::World).count(), 3);
    }

    #[test]
    fn vegetation_tool_declares_catalog_and_resident_field_boundaries() {
        assert!(VEGETATION_TOOL.requires(EditorSourceDomain::VegetationCatalog));
        assert!(VEGETATION_TOOL.requires(EditorSourceDomain::VegetationFields));
        assert_eq!(
            VEGETATION_TOOL.pinning,
            PinningPolicy::CatalogDraftAndResidentFields
        );
        assert!(
            VEGETATION_TOOL
                .commands
                .contains(&EditorCommandKind::EditVegetationProfile)
        );
        assert!(VEGETATION_TOOL.invalidates.is_empty());
    }
}
