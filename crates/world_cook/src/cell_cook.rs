//! Turns compiled cells into runtime cells. A cook validates and indexes the project's
//! catalogs once (`CookCatalog`), then encodes each compiled cell on its own: its pages, what
//! they name, its leaf control tile and their hash.

use super::*;
use environment_compile::CompiledCell;

pub(crate) struct CookCatalog {
    /// The runtime's manifest and catalogs; its content hash covers everything a cell's pages
    /// depend on besides the cell's own sources.
    pub header: RuntimeHeader,
    spaces: HashMap<WorldSpaceId, CookSpace>,
    layers: HashMap<(TerrainTextureSetId, TerrainSurfaceId), u16>,
    /// Each asset's LODs, in order.
    lods: HashMap<AssetId, Vec<u8>>,
    /// Each asset's height: its largest LOD bounds.
    heights: HashMap<AssetId, f32>,
    definitions: HashMap<ObjectDefinitionId, (Option<AssetId>, ObjectActivationPolicy)>,
}

/// What a cell's ground needs from its world.
struct CookSpace {
    cell_size: f32,
    texture_set: TerrainTextureSetId,
    weight_resolution: u16,
}

impl CookCatalog {
    /// Validates and indexes `project`'s global data; its cells, masks and objects are not
    /// read. `environment` fingerprints the compile plans every cell depends on.
    pub fn new(project: &ProjectDocument, environment: [u8; 32]) -> Result<Self> {
        if !project
            .world_spaces
            .iter()
            .any(|space| space.id == project.default_world_space)
        {
            bail!(
                "default world space {:?} is not present in the project",
                project.default_world_space
            );
        }

        let mut header = RuntimeHeader {
            manifest: RuntimeManifest {
                schema_version: RUNTIME_SCHEMA_VERSION,
                generation_id: String::new(),
                content_hash: [0; 32],
                default_world_space: project.default_world_space,
                world_spaces: project.world_spaces.clone(),
                vegetation_catalog: Some(project.vegetation_catalog.clone().unwrap_or_default()),
                // Outside the hash: moving the start or repainting an area recooks nothing.
                start_view: project.start_view.clone(),
                gameplay_areas: project.gameplay_areas.clone(),
            },
            terrain_surfaces: project.terrain_surfaces.clone(),
            terrain_texture_sets: project.terrain_texture_sets.clone(),
            terrain_texture_layers: project.terrain_texture_layers.clone(),
            terrain_profiles: project.terrain_profiles.clone(),
            assets: Vec::new(),
            definitions: Vec::new(),
        };
        sort_catalogs(&mut header);
        let (spaces, layers) = index_terrain(&header)?;
        header
            .manifest
            .vegetation_catalog
            .as_ref()
            .unwrap()
            .validate()?;
        let (lods, heights) = index_assets(project)?;
        let definitions = index_definitions(project, &lods)?;
        header.assets = asset_records(project);
        header.definitions = definition_records(project);
        let hash = header_hash(&header, environment)?;
        header.manifest.content_hash = hash;
        header.manifest.generation_id = blake3::Hash::from_bytes(hash).to_hex()[..16].to_owned();
        Ok(Self {
            header,
            spaces,
            layers,
            lods,
            heights,
            definitions,
        })
    }

    /// One cell's runtime output from its source record, its placed objects and its compiled
    /// environment.
    pub fn encode_cell(
        &self,
        source: &SourceCellRecord,
        objects: &[SourceObjectRecord],
        compiled: CompiledCell,
    ) -> Result<RuntimeCell> {
        let space = self
            .spaces
            .get(&source.space)
            .context("cell of an unknown world space")?;
        let CompiledCell {
            terrain,
            objects: generated,
            ground,
            vegetation,
            wear,
            ..
        } = compiled;
        let heightfield = terrain.context("the compiler omitted the cell's ground")?;
        let layers = self.ground_layers(space, &ground)?;
        let control = world::leaf_control(
            &layers,
            &ground.weight_pages,
            wear.as_ref(),
            source.cell,
            space.cell_size,
        )
        .map_err(anyhow::Error::msg)?;
        let mut cell = Cell::new(source);
        let height = heightfield.height_bounds();
        cell.output.record.terrain_resolution = heightfield.resolution;
        cell.extend_bounds(height[0], height[1]);
        cell.page(
            PageDomain::Terrain,
            PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
                heightfield,
                surfaces: ground.surfaces.clone(),
                wear,
            }),
            0,
        )?;
        cell.output.surfaces = ground.surfaces;

        if !vegetation.fields.is_empty() {
            let catalog = self.header.manifest.vegetation_catalog.as_ref().unwrap();
            vegetation.validate(catalog)?;
            let coverage = vegetation.fields.iter().map(|f| f.coverage.len() as u64);
            let gpu_bytes = coverage.sum();
            cell.page(
                PageDomain::Vegetation,
                PagePayload::Vegetation(vegetation),
                gpu_bytes,
            )?;
        }

        self.add_objects(&mut cell, objects, generated)?;
        let control = world_db::encode_terrain_control(&control)?;
        cell.hasher.update(&control);
        cell.output.control = control;
        cell.output.content_hash = *cell.hasher.finalize().as_bytes();
        Ok(cell.output)
    }

    /// The texture layer of each of the cell's ground surfaces, after checking its weights.
    fn ground_layers(
        &self,
        space: &CookSpace,
        ground: &environment_compile::CompiledGround,
    ) -> Result<Vec<u16>> {
        let surfaces = &ground.surfaces;

        if surfaces.is_empty() || surfaces.len() > MAX_TERRAIN_SURFACES_PER_CELL {
            bail!("a cell has one to {MAX_TERRAIN_SURFACES_PER_CELL} ground surfaces");
        }

        let pages = if surfaces.len() == 1 {
            0
        } else {
            surfaces.len().div_ceil(4)
        };
        let bytes = usize::from(space.weight_resolution).pow(2) * 4;

        if ground.weight_pages.len() != pages
            || ground
                .weight_pages
                .iter()
                .any(|p| p.resolution != space.weight_resolution || p.rgba.len() != bytes)
        {
            bail!("a cell's ground weights do not match its surfaces or its world's resolution");
        }

        surfaces
            .iter()
            .map(|surface| {
                self.layers
                    .get(&(space.texture_set, *surface))
                    .copied()
                    .context("a ground surface is not in its world's texture set")
            })
            .collect()
    }

    /// Placed objects with a visual asset and generated ones as the static-object page, each
    /// naming every LOD of its asset; placed proximity objects as the gameplay-object page.
    fn add_objects(
        &self,
        cell: &mut Cell,
        objects: &[SourceObjectRecord],
        generated: Vec<StaticObjectInstance>,
    ) -> Result<()> {
        let mut instances = generated;
        let mut gameplay = Vec::new();

        for object in objects {
            let &(visual, activation) = self
                .definitions
                .get(&object.definition)
                .with_context(|| format!("object {:?} has no definition", object.id))?;

            if let Some(asset) = visual {
                instances.push(StaticObjectInstance {
                    generated: false,
                    id: object.id,
                    asset,
                    translation: object.local_translation,
                    yaw: object.yaw,
                    scale: object.scale,
                });
            }

            if activation == ObjectActivationPolicy::Proximity {
                gameplay.push(GameplayObjectInstance {
                    id: object.id,
                    definition: object.definition,
                    translation: object.local_translation,
                    yaw: object.yaw,
                    scale: object.scale,
                });

                if !cell.output.definitions.contains(&object.definition) {
                    cell.output.definitions.push(object.definition);
                }
            }
        }

        if !instances.is_empty() {
            instances.sort_by_key(|o| o.id);

            if instances.windows(2).any(|w| w[0].id == w[1].id) {
                bail!("generated/manual object identity collision");
            }

            for object in &instances {
                let height = *self
                    .heights
                    .get(&object.asset)
                    .context("an object's asset is not in the catalog")?;
                let base = object.translation[1];
                cell.extend_bounds(base, base + height * object.scale);

                if !cell.output.assets.iter().any(|(a, _)| *a == object.asset) {
                    let lods = &self.lods[&object.asset];
                    cell.output
                        .assets
                        .extend(lods.iter().map(|&lod| (object.asset, lod)));
                }
            }

            cell.page(
                PageDomain::StaticObjects,
                PagePayload::StaticObjects(StaticObjectsPage { instances }),
                0,
            )?;
        }

        if !gameplay.is_empty() {
            cell.page(
                PageDomain::GameplayObjects,
                PagePayload::GameplayObjects(GameplayObjectsPage {
                    instances: gameplay,
                }),
                0,
            )?;
        }

        Ok(())
    }
}

/// A cell being encoded, and the hash of what it holds so far.
struct Cell {
    output: RuntimeCell,
    hasher: blake3::Hasher,
}

impl Cell {
    fn new(source: &SourceCellRecord) -> Self {
        Self {
            output: RuntimeCell {
                record: RuntimeCellRecord {
                    space: source.space,
                    cell: source.cell,
                    minimum_y: f32::INFINITY,
                    maximum_y: f32::NEG_INFINITY,
                    domain_mask: 0,
                    source_revision: source.source_revision,
                    terrain_resolution: 0,
                },
                pages: Vec::new(),
                assets: Vec::new(),
                definitions: Vec::new(),
                surfaces: Vec::new(),
                control: Vec::new(),
                content_hash: [0; 32],
            },
            hasher: blake3::Hasher::new(),
        }
    }

    fn extend_bounds(&mut self, minimum: f32, maximum: f32) {
        let record = &mut self.output.record;
        record.minimum_y = record.minimum_y.min(minimum);
        record.maximum_y = record.maximum_y.max(maximum);
    }

    fn page(&mut self, domain: PageDomain, payload: PagePayload, gpu_bytes: u64) -> Result<()> {
        let record = &mut self.output.record;
        let key = PageKey {
            space: record.space,
            cell: record.cell,
            domain,
        };
        let decoded = encode_page_payload(&payload)?;
        let checksum = *blake3::hash(&decoded).as_bytes();
        record.domain_mask |= domain_bit(domain);
        self.hasher.update(&(domain as i64).to_le_bytes());
        self.hasher.update(&checksum);
        self.output.pages.push(EncodedPage {
            key,
            codec: PageCodec::Zstd,
            decoded_bytes: decoded.len() as u64,
            gpu_bytes_estimate: gpu_bytes,
            checksum,
            payload: zstd::stream::encode_all(decoded.as_slice(), 1)?,
        });
        Ok(())
    }
}

/// Canonical order, so the hash and the output do not depend on how sources were read.
fn sort_catalogs(header: &mut RuntimeHeader) {
    header.manifest.world_spaces.sort_by_key(|space| space.id);
    header.terrain_surfaces.sort_by_key(|surface| surface.id);
    header.terrain_texture_sets.sort_by_key(|set| set.id);
    header
        .terrain_texture_layers
        .sort_by_key(|layer| (layer.texture_set, layer.layer));
    header.terrain_profiles.sort_by_key(|profile| profile.space);
}

type SurfaceLayers = HashMap<(TerrainTextureSetId, TerrainSurfaceId), u16>;

/// Checks the terrain catalog and each world's profile; indexes what cells' ground needs.
fn index_terrain(
    header: &RuntimeHeader,
) -> Result<(HashMap<WorldSpaceId, CookSpace>, SurfaceLayers)> {
    let unique = |count: usize, keys: HashSet<&str>| keys.len() == count;
    let surfaces: HashSet<_> = header.terrain_surfaces.iter().map(|s| s.id).collect();

    if surfaces.len() != header.terrain_surfaces.len()
        || !unique(
            header.terrain_surfaces.len(),
            header
                .terrain_surfaces
                .iter()
                .map(|s| s.key.as_str())
                .collect(),
        )
    {
        bail!("terrain surface IDs and keys must be unique");
    }

    for surface in &header.terrain_surfaces {
        validate_terrain_surface(surface)?;
    }

    let texture_sets: HashSet<_> = header.terrain_texture_sets.iter().map(|s| s.id).collect();

    if texture_sets.len() != header.terrain_texture_sets.len()
        || !unique(
            header.terrain_texture_sets.len(),
            header
                .terrain_texture_sets
                .iter()
                .map(|s| s.key.as_str())
                .collect(),
        )
    {
        bail!("terrain texture-set IDs and keys must be unique");
    }

    for texture_set in &header.terrain_texture_sets {
        validate_terrain_texture_set(texture_set)?;
    }

    let mut layers = HashMap::new();
    let mut by_index = HashSet::new();
    let mut indices: BTreeMap<TerrainTextureSetId, Vec<u16>> = BTreeMap::new();

    for layer in &header.terrain_texture_layers {
        if !texture_sets.contains(&layer.texture_set) || !surfaces.contains(&layer.surface) {
            bail!("terrain texture layer references a missing texture set or surface");
        }

        if layers
            .insert((layer.texture_set, layer.surface), layer.layer)
            .is_some()
            || !by_index.insert((layer.texture_set, layer.layer))
        {
            bail!("terrain texture-set layers must be unique");
        }

        indices
            .entry(layer.texture_set)
            .or_default()
            .push(layer.layer);
    }

    for (texture_set, layers) in &indices {
        if layers
            .iter()
            .enumerate()
            .any(|(expected, layer)| usize::from(*layer) != expected)
        {
            bail!("terrain texture set {texture_set:?} layers must be contiguous from zero");
        }
    }

    let profiles: HashMap<_, _> = header
        .terrain_profiles
        .iter()
        .map(|profile| (profile.space, profile))
        .collect();

    if profiles.len() != header.terrain_profiles.len() {
        bail!("world spaces may have only one terrain profile");
    }

    let mut spaces = HashMap::new();

    for space in &header.manifest.world_spaces {
        let Some(profile) = profiles.get(&space.id) else {
            bail!("world space {:?} has no terrain profile", space.id);
        };
        validate_terrain_profile(profile)?;

        if !texture_sets.contains(&profile.texture_set) {
            bail!(
                "world space {:?} references a missing terrain texture set",
                space.id
            );
        }

        spaces.insert(
            space.id,
            CookSpace {
                cell_size: space.cell_size,
                texture_set: profile.texture_set,
                weight_resolution: profile.weight_resolution,
            },
        );
    }

    Ok((spaces, layers))
}

type AssetLods = HashMap<AssetId, Vec<u8>>;

/// Each asset's LODs, after checking their thresholds, and its height.
fn index_assets(project: &ProjectDocument) -> Result<(AssetLods, HashMap<AssetId, f32>)> {
    let assets: HashSet<_> = project.assets.iter().map(|asset| asset.id).collect();
    let mut variants: BTreeMap<AssetId, Vec<(u8, f32)>> = BTreeMap::new();
    let mut heights: HashMap<AssetId, f32> = HashMap::new();

    for variant in &project.asset_variants {
        if !assets.contains(&variant.asset) {
            bail!(
                "LOD{} references missing asset {:?}",
                variant.lod,
                variant.asset
            );
        }

        if !variant.minimum_screen_height.is_finite() {
            bail!(
                "asset {:?} LOD{} has a non-finite screen-height threshold",
                variant.asset,
                variant.lod
            );
        }

        variants
            .entry(variant.asset)
            .or_default()
            .push((variant.lod, variant.minimum_screen_height));
        let height = heights.entry(variant.asset).or_insert(variant.bounds[1]);
        *height = height.max(variant.bounds[1]);
    }

    for (asset, lods) in &mut variants {
        lods.sort_by_key(|lod| lod.0);

        if lods.windows(2).any(|pair| pair[1].1 > pair[0].1) {
            bail!("asset {asset:?} LOD thresholds must descend by LOD");
        }

        if lods.last().is_some_and(|variant| variant.1 != 0.0) {
            bail!("asset {asset:?} coarsest LOD threshold must be zero");
        }
    }

    let lods = variants
        .into_iter()
        .map(|(asset, lods)| (asset, lods.into_iter().map(|lod| lod.0).collect()))
        .collect();
    Ok((lods, heights))
}

/// Definitions by ID, after checking their visual assets and the assets of collections.
fn index_definitions(
    project: &ProjectDocument,
    lods: &AssetLods,
) -> Result<HashMap<ObjectDefinitionId, (Option<AssetId>, ObjectActivationPolicy)>> {
    for definition in &project.definitions {
        if let Some(asset) = definition.visual_asset
            && !lods.contains_key(&asset)
        {
            bail!(
                "definition {:?} names visual asset {asset:?}, which is missing or has no LODs",
                definition.id
            );
        }
    }

    for preset in &project.presets.presets {
        if let environment::PresetKind::AssetCollection(collection) = &preset.kind {
            for item in &collection.assets {
                let scene = project
                    .assets
                    .iter()
                    .find(|a| a.id == item.asset)
                    .is_some_and(|a| a.kind == "gltf-scene");

                if !scene || !lods.contains_key(&item.asset) {
                    bail!("collection assets need glTF scene LOD variants");
                }
            }
        }
    }

    Ok(project
        .definitions
        .iter()
        .map(|d| (d.id, (d.visual_asset, d.activation)))
        .collect())
}

fn asset_records(project: &ProjectDocument) -> Vec<AssetVariantRecord> {
    let kinds: HashMap<_, _> = project
        .assets
        .iter()
        .map(|asset| (asset.id, asset.kind.as_str()))
        .collect();
    let mut records: Vec<_> = project
        .asset_variants
        .iter()
        .map(|variant| AssetVariantRecord {
            asset: variant.asset,
            lod: variant.lod,
            kind: kinds[&variant.asset].to_owned(),
            uri: variant.uri.clone(),
            bounds: variant.bounds,
            gpu_bytes_estimate: variant.gpu_bytes_estimate,
            shadow_policy: variant.shadow_policy,
            minimum_screen_height: variant.minimum_screen_height,
        })
        .collect();
    records.sort_by_key(|record| (record.asset, record.lod));
    records
}

fn definition_records(project: &ProjectDocument) -> Vec<RuntimeObjectDefinition> {
    let mut records: Vec<_> = project
        .definitions
        .iter()
        .map(|definition| RuntimeObjectDefinition {
            id: definition.id,
            key: definition.key.clone(),
            display_name: definition.display_name.clone(),
            visual_asset: definition.visual_asset,
            activation: definition.activation,
        })
        .collect();
    records.sort_by_key(|record| record.id);
    records
}

/// Everything the pages depend on besides each cell's own sources, in a fixed order.
fn header_hash(header: &RuntimeHeader, environment: [u8; 32]) -> Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let manifest = &header.manifest;
    hasher.update(&RUNTIME_SCHEMA_VERSION.to_le_bytes());
    hasher.update(&manifest.default_world_space.0.to_le_bytes());

    for space in &manifest.world_spaces {
        space.atmosphere.validate().map_err(anyhow::Error::msg)?;
        hasher.update(&space.id.0.to_le_bytes());
        hasher.update(space.name.as_bytes());

        for value in [space.cell_size, space.minimum_y, space.maximum_y] {
            hasher.update(&value.to_bits().to_le_bytes());
        }

        hasher.update(&world::codec::encode(&space.atmosphere)?);
    }

    hash_terrain_catalog(&mut hasher, header);
    hasher.update(&environment);
    hasher.update(&world::codec::encode(&manifest.vegetation_catalog)?);

    for asset in &header.assets {
        hasher.update(&asset.asset.0);
        hasher.update(&[asset.lod]);
        hasher.update(asset.kind.as_bytes());
        hasher.update(asset.uri.as_bytes());

        for bound in asset.bounds {
            hasher.update(&bound.to_bits().to_le_bytes());
        }

        hasher.update(&asset.gpu_bytes_estimate.to_le_bytes());
        hasher.update(&asset.shadow_policy.to_le_bytes());
        hasher.update(&asset.minimum_screen_height.to_bits().to_le_bytes());
    }

    for definition in &header.definitions {
        hasher.update(&definition.id.0);
        hasher.update(definition.key.as_bytes());
        hasher.update(definition.display_name.as_bytes());
        hasher.update(&[definition.activation as u8]);

        if let Some(asset) = definition.visual_asset {
            hasher.update(&asset.0);
        }
    }

    Ok(*hasher.finalize().as_bytes())
}

fn hash_terrain_catalog(hasher: &mut blake3::Hasher, header: &RuntimeHeader) {
    for surface in &header.terrain_surfaces {
        hasher.update(&surface.id.0);
        hasher.update(surface.key.as_bytes());
        hasher.update(surface.display_name.as_bytes());
        hasher.update(&[u8::from(surface.anti_tiling)]);

        for value in [
            surface.tile_size,
            surface.normal_y_sign,
            surface.normal_strength,
            surface.roughness_min,
            surface.roughness_max,
            surface.soft_depth,
            surface.macro_strength,
        ] {
            hasher.update(&value.to_bits().to_le_bytes());
        }
    }

    for texture_set in &header.terrain_texture_sets {
        hasher.update(&texture_set.id.0);
        hasher.update(texture_set.key.as_bytes());

        for uri in texture_set_uris(texture_set) {
            hasher.update(uri.as_bytes());
        }

        hasher.update(&texture_set.universal_gpu_bytes.to_le_bytes());
        hasher.update(&texture_set.astc_gpu_bytes.to_le_bytes());
    }

    for layer in &header.terrain_texture_layers {
        hasher.update(&layer.texture_set.0);
        hasher.update(&layer.surface.0);
        hasher.update(&layer.layer.to_le_bytes());
    }

    for profile in &header.terrain_profiles {
        hasher.update(&profile.space.0.to_le_bytes());
        hasher.update(&profile.texture_set.0);
        hasher.update(&profile.weight_resolution.to_le_bytes());

        for value in profile.macro_scales {
            hasher.update(&value.to_bits().to_le_bytes());
        }

        hasher.update(&profile.macro_contrast.to_bits().to_le_bytes());
        hasher.update(&profile.macro_albedo_strength.to_bits().to_le_bytes());
    }
}

fn texture_set_uris(texture_set: &TerrainTextureSet) -> [&String; 6] {
    [
        &texture_set.base_color_universal_uri,
        &texture_set.normal_material_universal_uri,
        &texture_set.macro_variation_universal_uri,
        &texture_set.base_color_astc_uri,
        &texture_set.normal_material_astc_uri,
        &texture_set.macro_variation_astc_uri,
    ]
}

fn validate_terrain_surface(surface: &TerrainSurface) -> Result<()> {
    if surface.key.is_empty()
        || surface.display_name.is_empty()
        || !surface.tile_size.is_finite()
        || surface.tile_size <= 0.0
        || !surface.normal_y_sign.is_finite()
        || surface.normal_y_sign.abs() != 1.0
        || !surface.normal_strength.is_finite()
        || surface.normal_strength < 0.0
        || !surface.roughness_min.is_finite()
        || !surface.roughness_max.is_finite()
        || !(0.0..=1.0).contains(&surface.roughness_min)
        || !(surface.roughness_min..=1.0).contains(&surface.roughness_max)
        || !(0.0..=2.0).contains(&surface.soft_depth)
        || !(0.0..=1.0).contains(&surface.macro_strength)
    {
        bail!("terrain surface {:?} is invalid", surface.id);
    }

    Ok(())
}

fn validate_terrain_profile(profile: &TerrainProfile) -> Result<()> {
    if profile.weight_resolution < 2
        || profile.weight_resolution > MAX_TERRAIN_WEIGHT_RESOLUTION
        || profile
            .macro_scales
            .into_iter()
            .any(|scale| !scale.is_finite() || scale <= 0.0)
        || profile
            .macro_scales
            .windows(2)
            .any(|pair| pair[1] <= pair[0])
        || !profile.macro_contrast.is_finite()
        || profile.macro_contrast < 0.0
        || !profile.macro_albedo_strength.is_finite()
        || !(0.0..=0.5).contains(&profile.macro_albedo_strength)
    {
        bail!("terrain profile for {:?} is invalid", profile.space);
    }

    Ok(())
}

fn validate_terrain_texture_set(texture_set: &TerrainTextureSet) -> Result<()> {
    if texture_set.key.is_empty() || texture_set_uris(texture_set).iter().any(|u| u.is_empty()) {
        bail!("terrain texture set {:?} is invalid", texture_set.id);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_profile_validation_matches_sqlite_bounds() {
        let mut profile = TerrainProfile {
            space: WorldSpaceId(1),
            texture_set: TerrainTextureSetId([1; 16]),
            weight_resolution: MAX_TERRAIN_WEIGHT_RESOLUTION,
            macro_scales: [7.7, 31.5, 235.0],
            macro_contrast: 0.0,
            macro_albedo_strength: 0.5,
        };
        assert!(validate_terrain_profile(&profile).is_ok());

        profile.weight_resolution = MAX_TERRAIN_WEIGHT_RESOLUTION + 1;
        assert!(validate_terrain_profile(&profile).is_err());

        profile.weight_resolution = MAX_TERRAIN_WEIGHT_RESOLUTION;
        profile.macro_albedo_strength = 0.500_1;
        assert!(validate_terrain_profile(&profile).is_err());
    }
}
