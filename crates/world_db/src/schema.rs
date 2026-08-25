pub const PROJECT_SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE project_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
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
    color_r REAL NOT NULL CHECK(color_r BETWEEN 0.0 AND 1.0),
    color_g REAL NOT NULL CHECK(color_g BETWEEN 0.0 AND 1.0),
    color_b REAL NOT NULL CHECK(color_b BETWEEN 0.0 AND 1.0),
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(world_space_id, cell_x, cell_z)
) STRICT, WITHOUT ROWID;

CREATE TABLE ground_cover_species (
    species_id BLOB PRIMARY KEY CHECK(length(species_id) = 16),
    species_key TEXT NOT NULL UNIQUE,
    bottom_color_r REAL NOT NULL CHECK(bottom_color_r BETWEEN 0.0 AND 1.0),
    bottom_color_g REAL NOT NULL CHECK(bottom_color_g BETWEEN 0.0 AND 1.0),
    bottom_color_b REAL NOT NULL CHECK(bottom_color_b BETWEEN 0.0 AND 1.0),
    top_color_r REAL NOT NULL CHECK(top_color_r BETWEEN 0.0 AND 1.0),
    top_color_g REAL NOT NULL CHECK(top_color_g BETWEEN 0.0 AND 1.0),
    top_color_b REAL NOT NULL CHECK(top_color_b BETWEEN 0.0 AND 1.0),
    minimum_card_height REAL NOT NULL CHECK(minimum_card_height > 0.0),
    maximum_card_height REAL NOT NULL CHECK(maximum_card_height >= minimum_card_height),
    minimum_card_width REAL NOT NULL CHECK(minimum_card_width > 0.0),
    maximum_card_width REAL NOT NULL CHECK(maximum_card_width >= minimum_card_width),
    flattened_card_probability REAL NOT NULL CHECK(flattened_card_probability BETWEEN 0.0 AND 1.0),
    maximum_wind_displacement REAL NOT NULL CHECK(maximum_wind_displacement >= 0.0)
) STRICT;

CREATE TABLE ground_cover_layers (
    layer_id BLOB PRIMARY KEY CHECK(length(layer_id) = 16),
    world_space_id INTEGER NOT NULL REFERENCES world_spaces(id),
    layer_key TEXT NOT NULL,
    species_id BLOB NOT NULL REFERENCES ground_cover_species(species_id),
    density_per_square_meter REAL NOT NULL CHECK(density_per_square_meter > 0.0),
    seed INTEGER NOT NULL CHECK(seed BETWEEN 0 AND 4294967295),
    UNIQUE(world_space_id, layer_key),
    UNIQUE(layer_id, world_space_id)
) STRICT;

CREATE TABLE ground_cover_cell_masks (
    layer_id BLOB NOT NULL,
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    resolution INTEGER NOT NULL CHECK(resolution BETWEEN 1 AND 64),
    coverage BLOB NOT NULL CHECK(length(coverage) = resolution * resolution),
    source_revision INTEGER NOT NULL CHECK(source_revision >= 0),
    PRIMARY KEY(layer_id, world_space_id, cell_x, cell_z),
    FOREIGN KEY(layer_id, world_space_id)
        REFERENCES ground_cover_layers(layer_id, world_space_id) ON DELETE CASCADE,
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

PRAGMA user_version = 6;
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

CREATE TABLE cell_pages (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    domain INTEGER NOT NULL CHECK(domain BETWEEN 1 AND 8),
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

CREATE TABLE ground_cover_species (
    species_id BLOB PRIMARY KEY CHECK(length(species_id) = 16),
    species_key TEXT NOT NULL UNIQUE,
    bottom_color_r REAL NOT NULL CHECK(bottom_color_r BETWEEN 0.0 AND 1.0),
    bottom_color_g REAL NOT NULL CHECK(bottom_color_g BETWEEN 0.0 AND 1.0),
    bottom_color_b REAL NOT NULL CHECK(bottom_color_b BETWEEN 0.0 AND 1.0),
    top_color_r REAL NOT NULL CHECK(top_color_r BETWEEN 0.0 AND 1.0),
    top_color_g REAL NOT NULL CHECK(top_color_g BETWEEN 0.0 AND 1.0),
    top_color_b REAL NOT NULL CHECK(top_color_b BETWEEN 0.0 AND 1.0),
    minimum_card_height REAL NOT NULL CHECK(minimum_card_height > 0.0),
    maximum_card_height REAL NOT NULL CHECK(maximum_card_height >= minimum_card_height),
    minimum_card_width REAL NOT NULL CHECK(minimum_card_width > 0.0),
    maximum_card_width REAL NOT NULL CHECK(maximum_card_width >= minimum_card_width),
    flattened_card_probability REAL NOT NULL CHECK(flattened_card_probability BETWEEN 0.0 AND 1.0),
    maximum_wind_displacement REAL NOT NULL CHECK(maximum_wind_displacement >= 0.0)
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

CREATE TABLE page_ground_cover_species (
    world_space_id INTEGER NOT NULL,
    cell_x INTEGER NOT NULL,
    cell_z INTEGER NOT NULL,
    domain INTEGER NOT NULL,
    lod INTEGER NOT NULL,
    species_id BLOB NOT NULL REFERENCES ground_cover_species(species_id),
    PRIMARY KEY(world_space_id, cell_x, cell_z, domain, lod, species_id),
    FOREIGN KEY(world_space_id, cell_x, cell_z, domain, lod)
        REFERENCES cell_pages(world_space_id, cell_x, cell_z, domain, lod)
) STRICT, WITHOUT ROWID;

PRAGMA user_version = 6;
"#;
