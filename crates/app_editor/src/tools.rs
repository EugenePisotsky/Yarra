//! Editor tools and the source data each one demands.
//!
//! A tool is not identified by a panel or an ECS entity. Its descriptor names the workspace it
//! belongs to and the project source the bounded query window loads while it is active.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::workspaces::EditorWorkspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EditorToolId(pub(crate) &'static str);

/// Source the project store's query window loads around the viewpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EditorSourceDomain {
    CellDescriptors,
    ObjectPlacements,
    EnvironmentCoverage,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EditorToolDescriptor {
    pub(crate) id: EditorToolId,
    pub(crate) label: &'static str,
    pub(crate) workspace: EditorWorkspace,
    pub(crate) source_domains: &'static [EditorSourceDomain],
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
    ],
};

pub(crate) const ROAD_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.roads"),
    label: "Roads",
    workspace: EditorWorkspace::World,
    source_domains: &[EditorSourceDomain::CellDescriptors],
};

/// Named places gameplay reacts to. The whole set is one small project record, loaded apart
/// from the spatial window.
pub(crate) const AREA_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.areas"),
    label: "Areas",
    workspace: EditorWorkspace::World,
    source_domains: &[],
};

pub(crate) const ENVIRONMENT_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.environment"),
    label: "Environment",
    workspace: EditorWorkspace::World,
    source_domains: &[
        EditorSourceDomain::CellDescriptors,
        EditorSourceDomain::EnvironmentCoverage,
    ],
};

pub(crate) const VEGETATION_TOOL: EditorToolDescriptor = EditorToolDescriptor {
    id: EditorToolId("world.vegetation"),
    label: "Vegetation",
    workspace: EditorWorkspace::World,
    source_domains: &[EditorSourceDomain::CellDescriptors],
};

pub(crate) fn object_tool_active(registry: Res<EditorToolRegistry>) -> bool {
    registry
        .active(EditorWorkspace::World)
        .is_some_and(|tool| tool.id == OBJECT_TOOL.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_keeps_one_active_tool_per_workspace() {
        let mut registry = EditorToolRegistry::default();
        registry.register(OBJECT_TOOL, true);
        registry.register(ENVIRONMENT_TOOL, false);
        registry.register(VEGETATION_TOOL, false);
        assert_eq!(
            registry.active(EditorWorkspace::World).map(|tool| tool.id),
            Some(OBJECT_TOOL.id)
        );
        assert!(registry.active(EditorWorkspace::Animation).is_none());
        assert!(registry.set_active(EditorWorkspace::World, ENVIRONMENT_TOOL.id));
        assert!(!registry.set_active(EditorWorkspace::World, ENVIRONMENT_TOOL.id));
        assert!(!registry.set_active(EditorWorkspace::Animation, OBJECT_TOOL.id));
        assert_eq!(
            registry.active(EditorWorkspace::World).map(|tool| tool.id),
            Some(ENVIRONMENT_TOOL.id)
        );
    }
}
