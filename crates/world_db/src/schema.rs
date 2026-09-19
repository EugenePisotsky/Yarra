pub const PROJECT_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE project_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE vegetation_catalog (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    format_version INTEGER NOT NULL CHECK(format_version = 7),
    payload BLOB NOT NULL
) STRICT;

CREATE TABLE world_spaces (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    cell_size REAL NOT NULL CHECK(cell_size > 0.0),
    minimum_y REAL NOT NULL,
    maximum_y REAL NOT NULL CHECK(maximum_y >= minimum_y)
) STRICT;

CREATE TABLE project_settings (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    default_world_space_id INTEGER NOT NULL REFERENCES world_spaces(id)
) STRICT;

CREATE TABLE source_cells (
    world_space_id INTEGER NOT NULL REFERENCES world_spaces(id),
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    height REAL NOT NULL,
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z)
) STRICT, WITHOUT ROWID;

CREATE TABLE terrain_surfaces (
    surface_id BLOB PRIMARY KEY CHECK(length(surface_id) = 16),
    surface_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    tile_size REAL NOT NULL CHECK(tile_size > 0.0),
    anti_tiling INTEGER NOT NULL CHECK(anti_tiling IN (0, 1)),
    normal_y_sign REAL NOT NULL CHECK(normal_y_sign IN (-1.0, 1.0)),
    normal_strength REAL NOT NULL CHECK(normal_strength BETWEEN 0.0 AND 1.0),
    roughness_min REAL NOT NULL CHECK(roughness_min BETWEEN 0.0 AND 1.0),
    roughness_max REAL NOT NULL CHECK(roughness_max BETWEEN roughness_min AND 1.0)
) STRICT;

CREATE TABLE terrain_texture_sets (
    texture_set_id BLOB PRIMARY KEY CHECK(length(texture_set_id) = 16),
    texture_set_key TEXT NOT NULL UNIQUE,
    base_color_universal_uri TEXT NOT NULL,
    normal_material_universal_uri TEXT NOT NULL,
    macro_variation_universal_uri TEXT NOT NULL,
    base_color_astc_uri TEXT NOT NULL,
    normal_material_astc_uri TEXT NOT NULL,
    macro_variation_astc_uri TEXT NOT NULL,
    universal_gpu_bytes INTEGER NOT NULL CHECK(universal_gpu_bytes >= 0),
    astc_gpu_bytes INTEGER NOT NULL CHECK(astc_gpu_bytes >= 0)
) STRICT;

CREATE TABLE terrain_texture_set_layers (
    texture_set_id BLOB NOT NULL REFERENCES terrain_texture_sets(texture_set_id) ON DELETE CASCADE,
    layer INTEGER NOT NULL CHECK(layer BETWEEN 0 AND 65535),
    surface_id BLOB NOT NULL REFERENCES terrain_surfaces(surface_id),
    PRIMARY KEY(texture_set_id, layer),
    UNIQUE(texture_set_id, surface_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE world_space_terrain_profiles (
    world_space_id INTEGER PRIMARY KEY REFERENCES world_spaces(id) ON DELETE CASCADE,
    texture_set_id BLOB NOT NULL REFERENCES terrain_texture_sets(texture_set_id),
    weight_resolution INTEGER NOT NULL CHECK(weight_resolution BETWEEN 2 AND 257),
    macro_small_scale REAL NOT NULL CHECK(macro_small_scale > 0.0),
    macro_medium_scale REAL NOT NULL CHECK(macro_medium_scale > 0.0),
    macro_large_scale REAL NOT NULL CHECK(macro_large_scale > 0.0),
    macro_contrast REAL NOT NULL CHECK(macro_contrast >= 0.0),
    macro_albedo_strength REAL NOT NULL CHECK(macro_albedo_strength BETWEEN 0.0 AND 0.5)
) STRICT;

CREATE TABLE terrain_cell_heightfields (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    resolution INTEGER NOT NULL CHECK(resolution BETWEEN 2 AND 257),
    heights BLOB NOT NULL CHECK(length(heights) = resolution * resolution * 4),
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z),
    FOREIGN KEY(world_space_id, cell_x, cell_z)
        REFERENCES source_cells(world_space_id, cell_x, cell_z) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- Small definition catalog; spatial masks live separately and are queried by cell.
CREATE TABLE environment_preset_state (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision > 0)
) STRICT;
CREATE TABLE environment_presets (
    preset_id BLOB PRIMARY KEY CHECK(length(preset_id) = 16),
    revision INTEGER NOT NULL CHECK(revision > 0),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 1048576)
) STRICT, WITHOUT ROWID;
CREATE TABLE environment_preset_dependencies (
    parent_id BLOB NOT NULL,
    child_id BLOB NOT NULL,
    PRIMARY KEY(parent_id, child_id),
    FOREIGN KEY(parent_id) REFERENCES environment_presets(preset_id) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY(child_id) REFERENCES environment_presets(preset_id) DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE INDEX environment_preset_parents ON environment_preset_dependencies(child_id, parent_id);
CREATE TABLE environment_layer_presets (
    world_space_id INTEGER NOT NULL,
    layer_id BLOB NOT NULL CHECK(length(layer_id) = 16),
    preset_id BLOB NOT NULL,
    PRIMARY KEY(world_space_id, layer_id),
    FOREIGN KEY(world_space_id) REFERENCES environment_definitions(world_space_id) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY(preset_id) REFERENCES environment_presets(preset_id) DEFERRABLE INITIALLY DEFERRED
) STRICT, WITHOUT ROWID;
CREATE INDEX environment_preset_uses ON environment_layer_presets(preset_id, world_space_id, layer_id);

CREATE TABLE environment_definitions (
    world_space_id INTEGER PRIMARY KEY REFERENCES world_spaces(id),
    revision INTEGER NOT NULL CHECK(revision > 0),
    payload BLOB NOT NULL CHECK(length(payload) <= 1048576)
) STRICT;

CREATE TABLE environment_cells (
    world_space_id INTEGER NOT NULL REFERENCES environment_definitions(world_space_id),
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z)
) STRICT, WITHOUT ROWID;

CREATE TABLE environment_coverage (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    layer_id BLOB NOT NULL CHECK(length(layer_id) = 16),
    samples BLOB NOT NULL CHECK(length(samples) BETWEEN 4 AND 66049),
    PRIMARY KEY(world_space_id, cell_x, cell_z, layer_id),
    FOREIGN KEY(world_space_id, cell_x, cell_z)
        REFERENCES environment_cells(world_space_id, cell_x, cell_z) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

-- Used when editing/deleting layer definitions, without loading every mask.
CREATE INDEX environment_coverage_layer
ON environment_coverage(world_space_id, layer_id, cell_x, cell_z);

CREATE TABLE source_assets (
    asset_id BLOB PRIMARY KEY CHECK(length(asset_id) = 32),
    asset_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    source_uri TEXT NOT NULL
) STRICT;

CREATE TABLE source_asset_variants (
    asset_id BLOB NOT NULL REFERENCES source_assets(asset_id) ON DELETE CASCADE,
    lod INTEGER NOT NULL CHECK(lod BETWEEN 0 AND 255),
    uri TEXT NOT NULL,
    bounds_x REAL NOT NULL CHECK(bounds_x >= 0.0),
    bounds_y REAL NOT NULL CHECK(bounds_y >= 0.0),
    bounds_z REAL NOT NULL CHECK(bounds_z >= 0.0),
    gpu_bytes_estimate INTEGER NOT NULL CHECK(gpu_bytes_estimate >= 0),
    shadow_policy INTEGER NOT NULL CHECK(shadow_policy BETWEEN 0 AND 2),
    minimum_screen_height REAL NOT NULL CHECK(minimum_screen_height >= 0.0),
    PRIMARY KEY(asset_id, lod)
) STRICT, WITHOUT ROWID;

CREATE TABLE object_definitions (
    definition_id BLOB PRIMARY KEY CHECK(length(definition_id) = 16),
    definition_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    visual_asset_id BLOB REFERENCES source_assets(asset_id),
    activation_policy INTEGER NOT NULL CHECK(activation_policy IN (0, 1))
) STRICT;

CREATE TABLE object_placements (
    object_id BLOB PRIMARY KEY CHECK(length(object_id) = 16),
    world_space_id INTEGER NOT NULL,
    owner_cell_x INTEGER NOT NULL,
    owner_cell_z INTEGER NOT NULL,
    definition_id BLOB NOT NULL REFERENCES object_definitions(definition_id),
    local_x REAL NOT NULL,
    local_y REAL NOT NULL,
    local_z REAL NOT NULL,
    yaw REAL NOT NULL,
    scale REAL NOT NULL CHECK(scale > 0.0),
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    FOREIGN KEY(world_space_id, owner_cell_x, owner_cell_z)
        REFERENCES source_cells(world_space_id, cell_x, cell_z)
) STRICT;

CREATE INDEX object_placements_owner
ON object_placements(world_space_id, owner_cell_x, owner_cell_z);

CREATE TABLE object_cell_overlaps (
    object_id BLOB NOT NULL REFERENCES object_placements(object_id) ON DELETE CASCADE,
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    PRIMARY KEY(object_id, world_space_id, cell_x, cell_z)
) STRICT, WITHOUT ROWID;

CREATE INDEX object_cell_overlaps_cells
ON object_cell_overlaps(world_space_id, cell_x, cell_z, object_id);

CREATE TABLE road_state (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    revision INTEGER NOT NULL CHECK(revision > 0)
) STRICT;
INSERT INTO road_state VALUES (1, 1);
-- Versions survive deletion. Reusing an identity requires the tombstone revision.
CREATE TABLE road_versions (
    kind INTEGER NOT NULL CHECK(kind BETWEEN 0 AND 4),
    id BLOB NOT NULL CHECK(length(id) = 16),
    revision INTEGER NOT NULL CHECK(revision > 0),
    PRIMARY KEY(kind, id)
) STRICT;
CREATE TABLE road_profiles (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    ground_preset BLOB NOT NULL REFERENCES environment_presets(preset_id) DEFERRABLE INITIALLY DEFERRED,
    payload BLOB NOT NULL CHECK(length(payload) <= 4096)
) STRICT;
CREATE INDEX road_profiles_ground ON road_profiles(ground_preset, id);
CREATE TABLE roads (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    world_space_id INTEGER NOT NULL REFERENCES environment_definitions(world_space_id) DEFERRABLE INITIALLY DEFERRED,
    profile_id BLOB NOT NULL REFERENCES road_profiles(id) DEFERRABLE INITIALLY DEFERRED,
    payload BLOB NOT NULL CHECK(length(payload) <= 4096)
) STRICT;
CREATE INDEX roads_profile ON roads(profile_id, world_space_id, id);
CREATE TABLE road_knots (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    road_id BLOB NOT NULL REFERENCES roads(id) DEFERRABLE INITIALLY DEFERRED,
    width REAL NOT NULL CHECK(width > 0 AND width <= 64),
    payload BLOB NOT NULL CHECK(length(payload) <= 4096),
    UNIQUE(id, road_id)
) STRICT;
CREATE INDEX road_knots_road ON road_knots(road_id, width, id);
CREATE TABLE road_spans (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    road_id BLOB NOT NULL REFERENCES roads(id) DEFERRABLE INITIALLY DEFERRED,
    start_id BLOB NOT NULL,
    end_id BLOB NOT NULL CHECK(start_id != end_id),
    min_x INTEGER NOT NULL,
    max_x INTEGER NOT NULL CHECK(max_x >= min_x),
    min_z INTEGER NOT NULL,
    max_z INTEGER NOT NULL CHECK(max_z >= min_z),
    payload BLOB NOT NULL CHECK(length(payload) <= 4096),
    UNIQUE(road_id, start_id),
    UNIQUE(road_id, end_id),
    FOREIGN KEY(start_id, road_id) REFERENCES road_knots(id, road_id) DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY(end_id, road_id) REFERENCES road_knots(id, road_id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX road_spans_start ON road_spans(start_id, id);
CREATE INDEX road_spans_end ON road_spans(end_id, id);
CREATE INDEX road_spans_road ON road_spans(road_id, id);
CREATE TABLE road_span_cells (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    span_id BLOB NOT NULL REFERENCES road_spans(id) ON DELETE CASCADE,
    PRIMARY KEY(world_space_id, cell_x, cell_z, span_id)
) STRICT;
CREATE INDEX road_span_cells_span ON road_span_cells(span_id);

CREATE TABLE road_junctions (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    world_space_id INTEGER NOT NULL REFERENCES environment_definitions(world_space_id) DEFERRABLE INITIALLY DEFERRED,
    profile_id BLOB NOT NULL REFERENCES road_profiles(id) DEFERRABLE INITIALLY DEFERRED,
    payload BLOB NOT NULL CHECK(length(payload) <= 4096)
) STRICT;
CREATE INDEX road_junctions_profile ON road_junctions(profile_id, world_space_id, id);
CREATE TABLE road_junction_knots (
    junction_id BLOB NOT NULL REFERENCES road_junctions(id) ON DELETE CASCADE,
    knot_id BLOB NOT NULL UNIQUE REFERENCES road_knots(id) DEFERRABLE INITIALLY DEFERRED,
    PRIMARY KEY(junction_id, knot_id)
) STRICT;
CREATE TABLE road_junction_cells (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    junction_id BLOB NOT NULL REFERENCES road_junctions(id) ON DELETE CASCADE,
    PRIMARY KEY(world_space_id, cell_x, cell_z, junction_id)
) STRICT;
CREATE INDEX road_junction_cells_id ON road_junction_cells(junction_id);

PRAGMA user_version = 22;
"#;

pub const RUNTIME_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = DELETE;

CREATE TABLE world_spaces (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    cell_size REAL NOT NULL CHECK(cell_size > 0.0),
    minimum_y REAL NOT NULL,
    maximum_y REAL NOT NULL CHECK(maximum_y >= minimum_y)
) STRICT;

CREATE TABLE runtime_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL,
    generation_id TEXT NOT NULL,
    content_hash BLOB NOT NULL CHECK(length(content_hash) = 32),
    default_world_space_id INTEGER NOT NULL REFERENCES world_spaces(id)
) STRICT;

CREATE TABLE vegetation_catalog (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    format_version INTEGER NOT NULL CHECK(format_version = 7),
    payload BLOB NOT NULL
) STRICT;

CREATE TABLE cells (
    world_space_id INTEGER NOT NULL REFERENCES world_spaces(id),
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    minimum_y REAL NOT NULL,
    maximum_y REAL NOT NULL CHECK(maximum_y >= minimum_y),
    domain_mask INTEGER NOT NULL CHECK(domain_mask >= 0),
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z)
) STRICT, WITHOUT ROWID;

CREATE TABLE terrain_surfaces (
    surface_id BLOB PRIMARY KEY CHECK(length(surface_id) = 16),
    surface_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    tile_size REAL NOT NULL CHECK(tile_size > 0.0),
    anti_tiling INTEGER NOT NULL CHECK(anti_tiling IN (0, 1)),
    normal_y_sign REAL NOT NULL CHECK(normal_y_sign IN (-1.0, 1.0)),
    normal_strength REAL NOT NULL CHECK(normal_strength BETWEEN 0.0 AND 1.0),
    roughness_min REAL NOT NULL CHECK(roughness_min BETWEEN 0.0 AND 1.0),
    roughness_max REAL NOT NULL CHECK(roughness_max BETWEEN roughness_min AND 1.0)
) STRICT;

CREATE TABLE terrain_texture_sets (
    texture_set_id BLOB PRIMARY KEY CHECK(length(texture_set_id) = 16),
    texture_set_key TEXT NOT NULL UNIQUE,
    base_color_universal_uri TEXT NOT NULL,
    normal_material_universal_uri TEXT NOT NULL,
    macro_variation_universal_uri TEXT NOT NULL,
    base_color_astc_uri TEXT NOT NULL,
    normal_material_astc_uri TEXT NOT NULL,
    macro_variation_astc_uri TEXT NOT NULL,
    universal_gpu_bytes INTEGER NOT NULL CHECK(universal_gpu_bytes >= 0),
    astc_gpu_bytes INTEGER NOT NULL CHECK(astc_gpu_bytes >= 0)
) STRICT;

CREATE TABLE terrain_texture_set_layers (
    texture_set_id BLOB NOT NULL REFERENCES terrain_texture_sets(texture_set_id) ON DELETE CASCADE,
    layer INTEGER NOT NULL CHECK(layer BETWEEN 0 AND 65535),
    surface_id BLOB NOT NULL REFERENCES terrain_surfaces(surface_id),
    PRIMARY KEY(texture_set_id, layer),
    UNIQUE(texture_set_id, surface_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE world_space_terrain_profiles (
    world_space_id INTEGER PRIMARY KEY REFERENCES world_spaces(id) ON DELETE CASCADE,
    texture_set_id BLOB NOT NULL REFERENCES terrain_texture_sets(texture_set_id),
    weight_resolution INTEGER NOT NULL CHECK(weight_resolution BETWEEN 2 AND 257),
    macro_small_scale REAL NOT NULL CHECK(macro_small_scale > 0.0),
    macro_medium_scale REAL NOT NULL CHECK(macro_medium_scale > 0.0),
    macro_large_scale REAL NOT NULL CHECK(macro_large_scale > 0.0),
    macro_contrast REAL NOT NULL CHECK(macro_contrast >= 0.0),
    macro_albedo_strength REAL NOT NULL CHECK(macro_albedo_strength BETWEEN 0.0 AND 0.5)
) STRICT;

CREATE TABLE cell_pages (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    domain INTEGER NOT NULL CHECK(domain IN (1, 2, 3, 5, 6, 7, 8, 9)),
    lod INTEGER NOT NULL CHECK(lod BETWEEN 0 AND 255),
    codec INTEGER NOT NULL CHECK(codec IN (0, 1)),
    encoded_bytes INTEGER NOT NULL CHECK(encoded_bytes >= 0),
    decoded_bytes INTEGER NOT NULL CHECK(decoded_bytes >= 0),
    gpu_bytes_estimate INTEGER NOT NULL CHECK(gpu_bytes_estimate >= 0),
    checksum BLOB NOT NULL CHECK(length(checksum) = 32),
    payload BLOB NOT NULL,
    PRIMARY KEY(world_space_id, cell_x, cell_z, domain, lod),
    FOREIGN KEY(world_space_id, cell_x, cell_z)
        REFERENCES cells(world_space_id, cell_x, cell_z)
) STRICT, WITHOUT ROWID;

CREATE TABLE asset_variants (
    asset_id BLOB NOT NULL CHECK(length(asset_id) = 32),
    lod INTEGER NOT NULL CHECK(lod BETWEEN 0 AND 255),
    kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    bounds_x REAL NOT NULL CHECK(bounds_x >= 0.0),
    bounds_y REAL NOT NULL CHECK(bounds_y >= 0.0),
    bounds_z REAL NOT NULL CHECK(bounds_z >= 0.0),
    gpu_bytes_estimate INTEGER NOT NULL CHECK(gpu_bytes_estimate >= 0),
    shadow_policy INTEGER NOT NULL CHECK(shadow_policy BETWEEN 0 AND 2),
    minimum_screen_height REAL NOT NULL CHECK(minimum_screen_height >= 0.0),
    PRIMARY KEY(asset_id, lod)
) STRICT, WITHOUT ROWID;

CREATE TABLE object_definitions (
    definition_id BLOB PRIMARY KEY CHECK(length(definition_id) = 16),
    definition_key TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    visual_asset_id BLOB CHECK(visual_asset_id IS NULL OR length(visual_asset_id) = 32),
    activation_policy INTEGER NOT NULL CHECK(activation_policy IN (0, 1))
) STRICT;

CREATE TABLE page_dependencies (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    domain INTEGER NOT NULL,
    lod INTEGER NOT NULL,
    asset_id BLOB NOT NULL,
    asset_lod INTEGER NOT NULL,
    PRIMARY KEY(
        world_space_id,
        cell_x,
        cell_z,
        domain,
        lod,
        asset_id,
        asset_lod
    ),
    FOREIGN KEY(world_space_id, cell_x, cell_z, domain, lod)
        REFERENCES cell_pages(world_space_id, cell_x, cell_z, domain, lod),
    FOREIGN KEY(asset_id, asset_lod)
        REFERENCES asset_variants(asset_id, lod)
) STRICT, WITHOUT ROWID;

CREATE TABLE page_object_definitions (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    domain INTEGER NOT NULL,
    lod INTEGER NOT NULL,
    definition_id BLOB NOT NULL REFERENCES object_definitions(definition_id),
    PRIMARY KEY(world_space_id, cell_x, cell_z, domain, lod, definition_id),
    FOREIGN KEY(world_space_id, cell_x, cell_z, domain, lod)
        REFERENCES cell_pages(world_space_id, cell_x, cell_z, domain, lod)
) STRICT, WITHOUT ROWID;

CREATE TABLE page_terrain_surfaces (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    domain INTEGER NOT NULL,
    lod INTEGER NOT NULL,
    surface_id BLOB NOT NULL REFERENCES terrain_surfaces(surface_id),
    PRIMARY KEY(world_space_id, cell_x, cell_z, domain, lod, surface_id),
    FOREIGN KEY(world_space_id, cell_x, cell_z, domain, lod)
        REFERENCES cell_pages(world_space_id, cell_x, cell_z, domain, lod)
) STRICT, WITHOUT ROWID;

CREATE TABLE terrain_nodes (
    world_space_id INTEGER NOT NULL REFERENCES world_spaces(id),
    level INTEGER NOT NULL CHECK(level BETWEEN 0 AND 30),
    node_x INTEGER NOT NULL, node_z INTEGER NOT NULL,
    parent_x INTEGER, parent_z INTEGER,
    child_mask INTEGER NOT NULL CHECK(child_mask BETWEEN 0 AND 15),
    minimum_y REAL NOT NULL, maximum_y REAL NOT NULL CHECK(maximum_y >= minimum_y),
    geometric_error REAL NOT NULL CHECK(geometric_error >= 0),
    resolution INTEGER CHECK(resolution BETWEEN 2 AND 257),
    codec INTEGER NOT NULL CHECK(codec IN (0,1)),
    decoded_bytes INTEGER NOT NULL CHECK(decoded_bytes BETWEEN 1 AND 1048576),
    gpu_bytes_estimate INTEGER NOT NULL CHECK(gpu_bytes_estimate >= 0),
    checksum BLOB NOT NULL CHECK(length(checksum)=32),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 1048576),
    PRIMARY KEY(world_space_id,level,node_x,node_z)
) STRICT, WITHOUT ROWID;
CREATE TABLE terrain_roots (
    world_space_id INTEGER NOT NULL, level INTEGER NOT NULL,
    node_x INTEGER NOT NULL, node_z INTEGER NOT NULL,
    PRIMARY KEY(world_space_id,level,node_x,node_z),
    FOREIGN KEY(world_space_id,level,node_x,node_z) REFERENCES terrain_nodes(world_space_id,level,node_x,node_z)
) STRICT, WITHOUT ROWID;
CREATE TABLE terrain_hierarchy_spaces (
    world_space_id INTEGER PRIMARY KEY REFERENCES world_spaces(id),
    node_count INTEGER NOT NULL CHECK(node_count >= 0),
    leaf_count INTEGER NOT NULL CHECK(leaf_count >= 0),
    root_count INTEGER NOT NULL CHECK(root_count BETWEEN 0 AND 256)
) STRICT;

CREATE TABLE terrain_composites (
    world_space_id INTEGER NOT NULL, level INTEGER NOT NULL,
    node_x INTEGER NOT NULL, node_z INTEGER NOT NULL,
    fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),
    checksum BLOB NOT NULL CHECK(length(checksum)=32),
    decoded_bytes INTEGER NOT NULL CHECK(decoded_bytes BETWEEN 1 AND 65536),
    gpu_bytes INTEGER NOT NULL CHECK(gpu_bytes=54432),
    payload BLOB NOT NULL CHECK(length(payload) BETWEEN 1 AND 65536),
    PRIMARY KEY(world_space_id,level,node_x,node_z),
    FOREIGN KEY(world_space_id,level,node_x,node_z) REFERENCES terrain_nodes(world_space_id,level,node_x,node_z)
) STRICT, WITHOUT ROWID;
CREATE TABLE terrain_material_spaces (
    world_space_id INTEGER PRIMARY KEY REFERENCES world_spaces(id),
    tile_count INTEGER NOT NULL CHECK(tile_count>=0)
) STRICT;
PRAGMA user_version = 18;
"#;
