mod actor;
mod camera_input_diagnostics;
pub use camera_input_diagnostics::CameraInputDiagnostics;
mod terrain_raycast;
pub use terrain_raycast::raycast_resident_terrain;
mod character;
mod character_catalog;
mod msaa_store;
mod object_lod;
pub use object_lod::ObjectLodPlugin;
mod world_streaming;
mod world_vegetation;
pub use world_vegetation::{
    GroundCanopyPlugin, GroundCanopySystems, GroundCanopyTiles, WorldVegetationPlugin,
    WorldVegetationSystems,
};

pub use atmosphere::clouds::CloudQuality;
pub use atmosphere::{
    ApplyAtmosphere, AtmosphereOwner, AtmospherePresentation, AtmosphereState,
    WorldEnvironmentCamera, WorldEnvironmentPlugin, WorldEnvironmentView, WorldSun,
};
pub use character::{
    CharacterPresentationPreview, CharacterPresentationPreviewPlugin, CharacterPreviewClip,
};
pub use character_catalog::{
    CharacterMovementContextDefinition, CharacterPresentationCatalogSummary,
    CharacterPresentationProfileSummary, CharacterPreviewClipDefinition, CharacterPreviewClipRole,
    DEFAULT_CHARACTER_PRESENTATION_ID, load_character_presentation_catalog_summary,
};
pub use msaa_store::{MsaaColorStorePlugin, MsaaColorStorePolicy};
pub use world_streaming::{
    ActiveWorldSpace, GameplayObject, GeneratedEnvironmentObject, LiveTerrainPreview,
    StreamedTerrainSurface, StreamedVegetationFieldPage, StreamedVisualObject,
    StreamingSmokePlugin, StreamingStats, TerrainContactReadiness, TerrainLodPreview,
    TerrainLodStats, TerrainPreviewRequest, VisualLodScale, WorldCatalog, WorldDebugControls,
    WorldDetailDemand, WorldGenerationReload, WorldOrigin, WorldRenderRoot, WorldSpaceInfo,
    WorldStreamingConfig, WorldStreamingPlugin, WorldStreamingSystems, WorldViewCamera,
    WorldViewpoint, sample_resident_terrain_surface, spawn_collection_visual,
};

mod start_view;
pub use start_view::WorldStartView;
mod gameplay;
pub use gameplay::{
    GAME_DEPTH_PREPASS_ENABLED, GameCameraPlugin, GameInputEnabled, GameInputPlugin,
    GamePointerInputBlocked, GameplayPlugin, GameplayPlugins, GameplaySystems, MinimalGamePlugin,
    MovementTargetPlugin,
};
