mod actor;
mod camera_input_diagnostics;
pub use camera_input_diagnostics::CameraInputDiagnostics;
mod terrain_raycast;
pub use terrain_raycast::raycast_resident_terrain;
mod character;
mod character_catalog;
mod msaa_store;
mod object_lod;
pub use object_lod::{ObjectFootprint, ObjectLodPlugin};
mod forest_shadow;
mod lod_lab;
pub use lod_lab::{
    LabAsset, LabBand, LabRepresentation, LabTree, LabVariant, LodLabPlugin, spawn_lab_tree,
};
mod tree_impostor;
mod tree_wind;
pub use tree_wind::{
    TreeInstancing, TreeWindPlugin, TreeWindResponse, TreeWindSystems, TreeWindTuning,
    tree_gltf_plugin,
};
mod day_clock;
pub use day_clock::{GameDayClock, GameDayClockPlugin, clock_time};
mod lightning;
pub use lightning::GameLightning;
mod weather;
pub use weather::{GameWeather, GameWeatherPlugin, WeatherStart};
pub use world::weather::{WeatherKind, WeatherParams};
mod world_streaming;
mod world_vegetation;
pub use world_vegetation::{
    GroundCanopyPlugin, GroundCanopySystems, GroundCanopyTiles, WorldVegetationPlugin,
    WorldVegetationSystems,
};

pub use atmosphere::clouds::CloudQuality;
pub use atmosphere::forest_shadow::ForestSkyOcclusion;
pub use atmosphere::precipitation::PrecipitationPresentation;
pub use atmosphere::{
    ApplyAtmosphere, AtmosphereOwner, AtmospherePresentation, AtmosphereState, FogTuning,
    WORLD_TONEMAPPING, WorldEnvironmentCamera, WorldEnvironmentPlugin, WorldEnvironmentView,
    WorldSun,
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

mod ocean;
pub use ocean::OceanPlugin;
mod start_view;
pub use start_view::{WORLD_VIEW_DISTANCE, WorldStartAdopted, WorldStartView};
mod gameplay;
pub use actor::{MoveIntent, PlayerControlled, TerrainGrounded};
pub use gameplay::{
    GameCameraPlugin, GameInputEnabled, GameInputPlugin, GamePointerInputBlocked, GameplayPlugin,
    GameplayPlugins, GameplaySystems, MinimalGamePlugin, MovementTargetPlugin, PlayerMovementSpeed,
    PlayerMovementSuspended, PlayerRoute, standing_character,
};
