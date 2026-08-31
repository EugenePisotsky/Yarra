pub const PROJECT_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE project_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE vegetation_catalog (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    format_version INTEGER NOT NULL CHECK(format_version = 1),
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

CREATE TABLE terrain_cell_surface_slots (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    slot INTEGER NOT NULL CHECK(slot BETWEEN 0 AND 7),
    surface_id BLOB NOT NULL REFERENCES terrain_surfaces(surface_id),
    PRIMARY KEY(world_space_id, cell_x, cell_z, slot),
    UNIQUE(world_space_id, cell_x, cell_z, surface_id),
    FOREIGN KEY(world_space_id, cell_x, cell_z)
        REFERENCES source_cells(world_space_id, cell_x, cell_z) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

CREATE TABLE terrain_cell_weight_pages (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    page INTEGER NOT NULL CHECK(page BETWEEN 0 AND 1),
    resolution INTEGER NOT NULL CHECK(resolution BETWEEN 2 AND 257),
    rgba BLOB NOT NULL CHECK(length(rgba) = resolution * resolution * 4),
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z, page),
    FOREIGN KEY(world_space_id, cell_x, cell_z)
        REFERENCES source_cells(world_space_id, cell_x, cell_z) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

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

CREATE TABLE vegetation_field_pages (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    format_version INTEGER NOT NULL CHECK(format_version = 1),
    payload BLOB NOT NULL,
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z),
    FOREIGN KEY(world_space_id, cell_x, cell_z)
        REFERENCES source_cells(world_space_id, cell_x, cell_z) ON DELETE CASCADE
) STRICT, WITHOUT ROWID;

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

PRAGMA user_version = 12;
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
    format_version INTEGER NOT NULL CHECK(format_version = 1),
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

PRAGMA user_version = 10;
"#;
