//! Turns a project and its compiled environment into runtime pages. The whole-document reference
//! (`build_runtime`) and the staged cook (one call per batch) share it: validate and index the
//! sources, hash what is not a page, then encode each cell's pages in cell order.

use super::*;
use environment_cook::VegetationPage;

pub(crate) fn build_compiled_runtime(
    mut project: ProjectDocument,
    environment: CookedEnvironment,
) -> Result<RuntimeBuild> {
    validate_world_spaces(&project)?;
    sort_sources(&mut project);
    let output = {
        let sources = SourceIndex::new(&project, &environment)?;
        let mut output = RuntimeOutput::new(&project, &environment)?;
        for source_cell in &project.cells {
            output.add_cell(source_cell, &sources, &environment)?;
        }
        output
    };
    Ok(output.finish(project, environment))
}

fn validate_world_spaces(project: &ProjectDocument) -> Result<()> {
    if project.world_spaces.is_empty() {
        bail!("a project must contain at least one world space");
    }
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
    Ok(())
}

/// Canonical order, so the content hash and page order do not depend on how sources were read.
fn sort_sources(project: &mut ProjectDocument) {
    project.world_spaces.sort_by_key(|space| space.id);
    project.cells.sort_by_key(|cell| (cell.space, cell.cell));
    project.terrain_surfaces.sort_by_key(|surface| surface.id);
    project
        .terrain_texture_sets
        .sort_by_key(|texture_set| texture_set.id);
    project
        .terrain_texture_layers
        .sort_by_key(|layer| (layer.texture_set, layer.layer));
    project
        .terrain_profiles
        .sort_by_key(|profile| profile.space);
    project
        .terrain_cell_heightfields
        .sort_by_key(|heightfield| (heightfield.space, heightfield.cell));
    project.assets.sort_by_key(|asset| asset.id.0);
    project
        .asset_variants
        .sort_by_key(|variant| (variant.asset.0, variant.lod));
    project.definitions.sort_by_key(|definition| definition.id);
    project.objects.sort_by_key(|object| object.id.0);
}

type CellKey = (WorldSpaceId, CellCoord);

/// The validated sources, indexed by what each cell's pages look up.
struct SourceIndex<'a> {
    terrain_slots: BTreeMap<CellKey, Vec<&'a TerrainSlot>>,
    terrain_weights: BTreeMap<CellKey, Vec<&'a TerrainWeights>>,
    vegetation: HashMap<CellKey, &'a VegetationPage>,
    assets: AssetIndex<'a>,
    definitions: HashMap<ObjectDefinitionId, &'a SourceObjectDefinitionRecord>,
    objects: BTreeMap<CellKey, Vec<&'a SourceObjectRecord>>,
}

impl<'a> SourceIndex<'a> {
    fn new(project: &'a ProjectDocument, environment: &'a CookedEnvironment) -> Result<Self> {
        let source_cells: HashSet<_> = project
            .cells
            .iter()
            .map(|cell| (cell.space, cell.cell))
            .collect();
        let catalog = TerrainCatalogIndex::new(project)?;
        let terrain_slots = index_terrain_slots(environment, &source_cells, &catalog)?;
        let terrain_weights = index_terrain_weights(environment, &source_cells, &catalog)?;
        let heightfields = index_terrain_heightfields(project, &source_cells)?;
        validate_cell_terrain(project, &terrain_slots, &terrain_weights)?;
        validate_terrain_heightfield_borders(&heightfields)?;
        let vegetation = index_vegetation(environment, &source_cells)?;
        let assets = AssetIndex::new(project)?;
        let definitions = index_definitions(project, &assets)?;
        let mut objects: BTreeMap<CellKey, Vec<&SourceObjectRecord>> = BTreeMap::new();
        for object in &project.objects {
            objects
                .entry((object.space, object.owner_cell))
                .or_default()
                .push(object);
        }
        Ok(Self {
            terrain_slots,
            terrain_weights,
            vegetation,
            assets,
            definitions,
            objects,
        })
    }
}

/// Surfaces, texture sets, their layers and each world space's profile.
struct TerrainCatalogIndex<'a> {
    layers_by_surface: HashMap<(TerrainTextureSetId, TerrainSurfaceId), u16>,
    profiles_by_space: HashMap<WorldSpaceId, &'a TerrainProfile>,
}

impl<'a> TerrainCatalogIndex<'a> {
    fn new(project: &'a ProjectDocument) -> Result<Self> {
        let surfaces_by_id: HashMap<_, _> = project
            .terrain_surfaces
            .iter()
            .map(|surface| (surface.id, surface))
            .collect();
        if surfaces_by_id.len() != project.terrain_surfaces.len() {
            bail!("terrain surface IDs must be unique");
        }
        if project
            .terrain_surfaces
            .iter()
            .map(|surface| surface.key.as_str())
            .collect::<HashSet<_>>()
            .len()
            != project.terrain_surfaces.len()
        {
            bail!("terrain surface keys must be unique");
        }
        for surface in &project.terrain_surfaces {
            validate_terrain_surface(surface)?;
        }
        let texture_sets_by_id: HashMap<_, _> = project
            .terrain_texture_sets
            .iter()
            .map(|texture_set| (texture_set.id, texture_set))
            .collect();
        if texture_sets_by_id.len() != project.terrain_texture_sets.len() {
            bail!("terrain texture-set IDs must be unique");
        }
        if project
            .terrain_texture_sets
            .iter()
            .map(|texture_set| texture_set.key.as_str())
            .collect::<HashSet<_>>()
            .len()
            != project.terrain_texture_sets.len()
        {
            bail!("terrain texture-set keys must be unique");
        }
        for texture_set in &project.terrain_texture_sets {
            validate_terrain_texture_set(texture_set)?;
        }
        let mut layers_by_surface = HashMap::new();
        let mut layers_by_index = HashSet::new();
        let mut layer_indices_by_set: BTreeMap<TerrainTextureSetId, Vec<u16>> = BTreeMap::new();
        for layer in &project.terrain_texture_layers {
            if !texture_sets_by_id.contains_key(&layer.texture_set) {
                bail!("terrain texture layer references a missing texture set");
            }
            if !surfaces_by_id.contains_key(&layer.surface) {
                bail!("terrain texture layer references a missing surface");
            }
            if layers_by_surface
                .insert((layer.texture_set, layer.surface), layer.layer)
                .is_some()
                || !layers_by_index.insert((layer.texture_set, layer.layer))
            {
                bail!("terrain texture-set layers must be unique");
            }
            layer_indices_by_set
                .entry(layer.texture_set)
                .or_default()
                .push(layer.layer);
        }
        for (texture_set, layers) in &layer_indices_by_set {
            if layers
                .iter()
                .enumerate()
                .any(|(expected, layer)| usize::from(*layer) != expected)
            {
                bail!(
                    "terrain texture set {:?} layers must be contiguous from zero",
                    texture_set
                );
            }
        }
        let profiles_by_space: HashMap<_, _> = project
            .terrain_profiles
            .iter()
            .map(|profile| (profile.space, profile))
            .collect();
        if profiles_by_space.len() != project.terrain_profiles.len() {
            bail!("world spaces may have only one terrain profile");
        }
        for space in &project.world_spaces {
            let Some(profile) = profiles_by_space.get(&space.id) else {
                bail!("world space {:?} has no terrain profile", space.id);
            };
            validate_terrain_profile(profile)?;
            if !texture_sets_by_id.contains_key(&profile.texture_set) {
                bail!(
                    "world space {:?} references a missing terrain texture set",
                    space.id
                );
            }
        }
        Ok(Self {
            layers_by_surface,
            profiles_by_space,
        })
    }
}

fn index_terrain_slots<'a>(
    environment: &'a CookedEnvironment,
    source_cells: &HashSet<CellKey>,
    catalog: &TerrainCatalogIndex,
) -> Result<BTreeMap<CellKey, Vec<&'a TerrainSlot>>> {
    let mut slots_by_cell: BTreeMap<CellKey, Vec<&TerrainSlot>> = BTreeMap::new();
    for slot in &environment.slots {
        if !source_cells.contains(&(slot.space, slot.cell)) {
            bail!(
                "terrain surface slot references missing cell {:?}",
                slot.cell
            );
        }
        let Some(profile) = catalog.profiles_by_space.get(&slot.space) else {
            bail!("terrain surface slot references an unprofiled world space");
        };
        if !catalog
            .layers_by_surface
            .contains_key(&(profile.texture_set, slot.surface))
        {
            bail!("terrain surface slot is not present in the world's texture set");
        }
        slots_by_cell
            .entry((slot.space, slot.cell))
            .or_default()
            .push(slot);
    }
    Ok(slots_by_cell)
}

fn index_terrain_weights<'a>(
    environment: &'a CookedEnvironment,
    source_cells: &HashSet<CellKey>,
    catalog: &TerrainCatalogIndex,
) -> Result<BTreeMap<CellKey, Vec<&'a TerrainWeights>>> {
    let mut weights_by_cell: BTreeMap<CellKey, Vec<&TerrainWeights>> = BTreeMap::new();
    for weights in &environment.weights {
        if !source_cells.contains(&(weights.space, weights.cell)) {
            bail!(
                "terrain weight page references missing cell {:?}",
                weights.cell
            );
        }
        let profile = catalog.profiles_by_space[&weights.space];
        let expected_bytes = usize::from(weights.resolution).pow(2) * 4;
        if weights.resolution != profile.weight_resolution || weights.rgba.len() != expected_bytes {
            bail!("terrain weight page has the wrong resolution or byte count");
        }
        weights_by_cell
            .entry((weights.space, weights.cell))
            .or_default()
            .push(weights);
    }
    Ok(weights_by_cell)
}

fn index_terrain_heightfields<'a>(
    project: &'a ProjectDocument,
    source_cells: &HashSet<CellKey>,
) -> Result<BTreeMap<CellKey, &'a SourceTerrainCellHeightfieldRecord>> {
    let spaces_by_id: HashMap<_, _> = project
        .world_spaces
        .iter()
        .map(|space| (space.id, space))
        .collect();
    let mut heightfields_by_cell = BTreeMap::new();
    for heightfield in &project.terrain_cell_heightfields {
        if !source_cells.contains(&(heightfield.space, heightfield.cell)) {
            bail!(
                "terrain heightfield references missing cell {:?}",
                heightfield.cell
            );
        }
        let resolution = usize::from(heightfield.resolution);
        if !(2..=usize::from(MAX_TERRAIN_HEIGHTFIELD_RESOLUTION)).contains(&resolution)
            || heightfield.heights.len() != resolution * resolution
            || !heightfield.heights.iter().all(|height| height.is_finite())
            || heightfield.source_revision < 0
        {
            bail!(
                "terrain heightfield {:?} has invalid dimensions or samples",
                heightfield.cell
            );
        }
        let space = spaces_by_id[&heightfield.space];
        if heightfield
            .heights
            .iter()
            .any(|height| *height < space.minimum_y || *height > space.maximum_y)
        {
            bail!(
                "terrain heightfield {:?} exceeds world-space height bounds",
                heightfield.cell
            );
        }
        if heightfields_by_cell
            .insert((heightfield.space, heightfield.cell), heightfield)
            .is_some()
        {
            bail!("terrain cells may have only one heightfield");
        }
    }
    Ok(heightfields_by_cell)
}

/// Every source cell has contiguous surface slots and, unless constant, the matching number of
/// contiguous weight pages.
fn validate_cell_terrain(
    project: &ProjectDocument,
    slots_by_cell: &BTreeMap<CellKey, Vec<&TerrainSlot>>,
    weights_by_cell: &BTreeMap<CellKey, Vec<&TerrainWeights>>,
) -> Result<()> {
    for source_cell in &project.cells {
        let Some(slots) = slots_by_cell.get(&(source_cell.space, source_cell.cell)) else {
            bail!("terrain cell {:?} has no surface slots", source_cell.cell);
        };
        if slots.is_empty() || slots.len() > MAX_TERRAIN_SURFACES_PER_CELL {
            bail!(
                "terrain cell {:?} has an invalid surface count",
                source_cell.cell
            );
        }
        if slots
            .iter()
            .enumerate()
            .any(|(expected, slot)| usize::from(slot.slot) != expected)
        {
            bail!(
                "terrain cell {:?} surface slots must be contiguous",
                source_cell.cell
            );
        }
        let expected_weight_pages = slots.len().div_ceil(4);
        let actual_weight_pages = weights_by_cell
            .get(&(source_cell.space, source_cell.cell))
            .map_or(0, Vec::len);
        if weights_by_cell
            .get(&(source_cell.space, source_cell.cell))
            .is_some_and(|pages| {
                pages
                    .iter()
                    .enumerate()
                    .any(|(expected, page)| usize::from(page.page) != expected)
            })
        {
            bail!(
                "terrain cell {:?} weight pages must be contiguous",
                source_cell.cell
            );
        }
        if slots.len() == 1 {
            if actual_weight_pages != 0 {
                bail!("constant terrain cells must not store weight pages");
            }
        } else if actual_weight_pages != expected_weight_pages
            || actual_weight_pages > MAX_TERRAIN_WEIGHT_PAGES
        {
            bail!(
                "terrain cell {:?} has the wrong number of weight pages",
                source_cell.cell
            );
        }
    }
    Ok(())
}

fn index_vegetation<'a>(
    environment: &'a CookedEnvironment,
    source_cells: &HashSet<CellKey>,
) -> Result<HashMap<CellKey, &'a VegetationPage>> {
    let mut pages_by_cell = HashMap::new();
    if !environment.vegetation.is_empty() {
        let catalog = environment
            .catalog
            .as_ref()
            .context("vegetation field pages require a vegetation catalog")?;
        catalog.validate()?;
        for page in &environment.vegetation {
            if !source_cells.contains(&(page.space, page.cell)) {
                bail!("vegetation page references missing cell {:?}", page.cell);
            }
            if page.source_revision < 0 {
                bail!("vegetation page {:?} has a negative revision", page.cell);
            }
            page.data.validate(catalog)?;
            if pages_by_cell
                .insert((page.space, page.cell), page)
                .is_some()
            {
                bail!("vegetation cells may have only one field page");
            }
        }
    } else if let Some(catalog) = &environment.catalog {
        catalog.validate()?;
    }
    Ok(pages_by_cell)
}

/// Assets with their LOD thresholds and tallest variant.
struct AssetIndex<'a> {
    by_id: HashMap<AssetId, &'a SourceAssetRecord>,
    variants: HashMap<AssetId, Vec<(u8, f32)>>,
    maximum_height: HashMap<AssetId, f32>,
}

impl<'a> AssetIndex<'a> {
    fn new(project: &'a ProjectDocument) -> Result<Self> {
        let by_id: HashMap<_, _> = project
            .assets
            .iter()
            .map(|asset| (asset.id, asset))
            .collect();
        let mut variants: HashMap<AssetId, Vec<(u8, f32)>> = HashMap::new();
        let mut maximum_height: HashMap<AssetId, f32> = HashMap::new();
        for variant in &project.asset_variants {
            if !by_id.contains_key(&variant.asset) {
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
            maximum_height
                .entry(variant.asset)
                .and_modify(|height| *height = height.max(variant.bounds[1]))
                .or_insert(variant.bounds[1]);
        }
        for (asset, lods) in &variants {
            if lods.windows(2).any(|pair| pair[1].1 > pair[0].1) {
                bail!("asset {:?} LOD thresholds must descend by LOD", asset);
            }
            if lods.last().is_some_and(|variant| variant.1 != 0.0) {
                bail!("asset {:?} coarsest LOD threshold must be zero", asset);
            }
        }
        Ok(Self {
            by_id,
            variants,
            maximum_height,
        })
    }
}

/// Definitions by ID, after checking their visual assets, collection assets and that every
/// object's definition exists.
fn index_definitions<'a>(
    project: &'a ProjectDocument,
    assets: &AssetIndex,
) -> Result<HashMap<ObjectDefinitionId, &'a SourceObjectDefinitionRecord>> {
    let definitions_by_id: HashMap<_, _> = project
        .definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect();
    for definition in &project.definitions {
        let Some(asset) = definition.visual_asset else {
            continue;
        };
        if !assets.by_id.contains_key(&asset) {
            bail!(
                "definition {:?} references missing visual asset {:?}",
                definition.id,
                asset
            );
        }
        if !assets.variants.contains_key(&asset) {
            bail!("visual asset {:?} has no runtime LOD variants", asset);
        }
    }
    for preset in &project.presets.presets {
        if let environment::PresetKind::AssetCollection(c) = &preset.kind {
            for a in &c.assets {
                let source = assets
                    .by_id
                    .get(&a.asset)
                    .context("collection references missing asset")?;
                if source.kind != "gltf-scene" || !assets.variants.contains_key(&a.asset) {
                    bail!("collection assets need glTF scene LOD variants");
                }
            }
        }
    }
    for object in &project.objects {
        if !definitions_by_id.contains_key(&object.definition) {
            bail!(
                "object {:?} references missing definition {:?}",
                object.id,
                object.definition
            );
        }
    }
    Ok(definitions_by_id)
}

/// The runtime build in progress: cell records, pages and their dependencies, and the content
/// hash over the sources and every page in order.
struct RuntimeOutput {
    cells: Vec<RuntimeCellRecord>,
    pages: Vec<EncodedPage>,
    dependencies: Vec<PageDependencyRecord>,
    definition_dependencies: Vec<PageObjectDefinitionRecord>,
    terrain_surface_dependencies: Vec<PageTerrainSurfaceRecord>,
    content_hasher: blake3::Hasher,
}

impl RuntimeOutput {
    fn new(project: &ProjectDocument, environment: &CookedEnvironment) -> Result<Self> {
        let mut content_hasher = blake3::Hasher::new();
        hash_sources(&mut content_hasher, project, environment)?;
        Ok(Self {
            cells: Vec::with_capacity(project.cells.len()),
            pages: Vec::with_capacity(project.cells.len() * 3),
            dependencies: Vec::new(),
            definition_dependencies: Vec::new(),
            terrain_surface_dependencies: Vec::new(),
            content_hasher,
        })
    }

    fn push_page(&mut self, page: EncodedPage) {
        hash_page(&mut self.content_hasher, &page);
        self.pages.push(page);
    }

    /// One cell's pages (terrain, then vegetation, static and gameplay objects) and its record.
    fn add_cell(
        &mut self,
        source_cell: &SourceCellRecord,
        sources: &SourceIndex,
        environment: &CookedEnvironment,
    ) -> Result<()> {
        let cell_key = (source_cell.space, source_cell.cell);
        let (terrain_height_bounds, terrain_resolution) =
            self.add_terrain(source_cell, sources, environment)?;

        let source_objects = sources
            .objects
            .get(&cell_key)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut render_objects = Vec::new();
        let mut gameplay_objects = Vec::new();
        for object in source_objects {
            let definition = sources.definitions[&object.definition];
            if let Some(asset) = definition.visual_asset {
                render_objects.push((*object, asset));
            }
            if definition.activation == ObjectActivationPolicy::Proximity {
                gameplay_objects.push(*object);
            }
        }

        let mut domain_mask = domain_bit(PageDomain::Terrain);
        if let Some(source_page) = sources.vegetation.get(&cell_key) {
            self.add_vegetation(source_cell, source_page)?;
            domain_mask |= domain_bit(PageDomain::Vegetation);
        }
        let generated = environment
            .objects
            .get(&cell_key)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !render_objects.is_empty() || !generated.is_empty() {
            domain_mask |= domain_bit(PageDomain::StaticObjects);
            self.add_static_objects(source_cell, &render_objects, generated, sources)?;
        }
        if !gameplay_objects.is_empty() {
            domain_mask |= domain_bit(PageDomain::GameplayObjects);
            self.add_gameplay_objects(source_cell, &gameplay_objects)?;
        }

        let mut minimum_y = terrain_height_bounds[0];
        let mut maximum_y = terrain_height_bounds[1];
        let maximum_height = &sources.assets.maximum_height;
        for (object, asset) in &render_objects {
            minimum_y = minimum_y.min(object.local_translation[1]);
            maximum_y =
                maximum_y.max(object.local_translation[1] + maximum_height[asset] * object.scale);
        }
        for object in generated {
            minimum_y = minimum_y.min(object.translation[1]);
            maximum_y =
                maximum_y.max(object.translation[1] + maximum_height[&object.asset] * object.scale);
        }
        self.cells.push(RuntimeCellRecord {
            space: source_cell.space,
            cell: source_cell.cell,
            minimum_y,
            maximum_y,
            domain_mask,
            source_revision: source_cell.source_revision,
            terrain_resolution,
        });
        Ok(())
    }

    /// The cell's heightfield page; returns its height bounds and resolution.
    fn add_terrain(
        &mut self,
        source_cell: &SourceCellRecord,
        sources: &SourceIndex,
        environment: &CookedEnvironment,
    ) -> Result<([f32; 2], u16)> {
        let cell_key = (source_cell.space, source_cell.cell);
        let terrain_slots = sources.terrain_slots[&cell_key].as_slice();
        let terrain_weights = sources
            .terrain_weights
            .get(&cell_key)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let terrain_key = page_key(source_cell, PageDomain::Terrain);
        let surfaces = terrain_slots.iter().map(|slot| slot.surface).collect();
        let weight_pages = terrain_weights
            .iter()
            .map(|weights| TerrainWeightPage {
                resolution: weights.resolution,
                rgba: weights.rgba.clone(),
            })
            .collect();
        let heightfield = environment
            .terrain
            .get(&cell_key)
            .context("environment compiler omitted terrain")?
            .clone();
        let height_bounds = heightfield.height_bounds();
        let resolution = heightfield.resolution;
        let heightfield_gpu_bytes = estimate_heightfield_gpu_bytes(heightfield.resolution);
        let terrain_payload = PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
            heightfield,
            surfaces,
            weight_pages,
        });
        let terrain_page = encoded_page(
            terrain_key,
            terrain_payload,
            terrain_weights
                .iter()
                .map(|weights| weights.rgba.len() as u64)
                .sum::<u64>()
                + heightfield_gpu_bytes,
        )?;
        self.push_page(terrain_page);
        self.terrain_surface_dependencies
            .extend(terrain_slots.iter().map(|slot| PageTerrainSurfaceRecord {
                page: terrain_key,
                surface: slot.surface,
            }));
        Ok((height_bounds, resolution))
    }

    fn add_vegetation(
        &mut self,
        source_cell: &SourceCellRecord,
        source_page: &VegetationPage,
    ) -> Result<()> {
        let gpu_bytes_estimate = source_page
            .data
            .fields
            .iter()
            .map(|field| field.coverage.len() as u64)
            .sum();
        let page = encoded_page(
            page_key(source_cell, PageDomain::Vegetation),
            PagePayload::Vegetation(source_page.data.clone()),
            gpu_bytes_estimate,
        )?;
        self.push_page(page);
        Ok(())
    }

    /// Placed objects with a visual asset and generated ones, each depending on every LOD of
    /// its asset.
    fn add_static_objects(
        &mut self,
        source_cell: &SourceCellRecord,
        render_objects: &[(&SourceObjectRecord, AssetId)],
        generated: &[StaticObjectInstance],
        sources: &SourceIndex,
    ) -> Result<()> {
        let object_key = page_key(source_cell, PageDomain::StaticObjects);
        let mut instances: Vec<_> = render_objects
            .iter()
            .map(|(object, asset)| StaticObjectInstance {
                generated: false,
                id: object.id,
                asset: *asset,
                translation: object.local_translation,
                yaw: object.yaw,
                scale: object.scale,
            })
            .collect();
        instances.extend_from_slice(generated);
        instances.sort_by_key(|o| o.id);
        if instances.windows(2).any(|w| w[0].id == w[1].id) {
            bail!("generated/manual object identity collision");
        }
        let object_page = encoded_page(
            object_key,
            PagePayload::StaticObjects(StaticObjectsPage { instances }),
            0,
        )?;
        self.push_page(object_page);
        let mut depended_assets = HashSet::new();
        for asset in render_objects
            .iter()
            .map(|(_, a)| a)
            .chain(generated.iter().map(|o| &o.asset))
        {
            if !depended_assets.insert(*asset) {
                continue;
            }
            for (asset_lod, _) in &sources.assets.variants[asset] {
                self.dependencies.push(PageDependencyRecord {
                    page: object_key,
                    asset: *asset,
                    asset_lod: *asset_lod,
                });
            }
        }
        Ok(())
    }

    fn add_gameplay_objects(
        &mut self,
        source_cell: &SourceCellRecord,
        gameplay_objects: &[&SourceObjectRecord],
    ) -> Result<()> {
        let gameplay_key = page_key(source_cell, PageDomain::GameplayObjects);
        let instances = gameplay_objects
            .iter()
            .map(|object| GameplayObjectInstance {
                id: object.id,
                definition: object.definition,
                translation: object.local_translation,
                yaw: object.yaw,
                scale: object.scale,
            })
            .collect();
        let gameplay_page = encoded_page(
            gameplay_key,
            PagePayload::GameplayObjects(GameplayObjectsPage { instances }),
            0,
        )?;
        self.push_page(gameplay_page);
        let mut depended_definitions = HashSet::new();
        for object in gameplay_objects {
            if depended_definitions.insert(object.definition) {
                self.definition_dependencies
                    .push(PageObjectDefinitionRecord {
                        page: gameplay_key,
                        definition: object.definition,
                    });
            }
        }
        Ok(())
    }

    fn finish(self, project: ProjectDocument, environment: CookedEnvironment) -> RuntimeBuild {
        let kinds: HashMap<_, _> = project
            .assets
            .iter()
            .map(|asset| (asset.id, asset.kind.as_str()))
            .collect();
        let assets = project
            .asset_variants
            .into_iter()
            .map(|variant| AssetVariantRecord {
                asset: variant.asset,
                lod: variant.lod,
                kind: kinds[&variant.asset].to_owned(),
                uri: variant.uri,
                bounds: variant.bounds,
                gpu_bytes_estimate: variant.gpu_bytes_estimate,
                shadow_policy: variant.shadow_policy,
                minimum_screen_height: variant.minimum_screen_height,
            })
            .collect();
        let definitions = project
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
        let content_hash = *self.content_hasher.finalize().as_bytes();
        let generation_id = blake3::Hash::from_bytes(content_hash).to_hex()[..16].to_owned();

        RuntimeBuild {
            manifest: RuntimeManifest {
                schema_version: RUNTIME_SCHEMA_VERSION,
                generation_id,
                content_hash,
                default_world_space: project.default_world_space,
                world_spaces: project.world_spaces,
                vegetation_catalog: environment.catalog,
                // Not hashed: moving the start never recooks the world.
                start_view: project.start_view,
                // Not hashed either: repainting an area never recooks the world.
                gameplay_areas: project.gameplay_areas,
            },
            cells: self.cells,
            pages: self.pages,
            terrain_surfaces: project.terrain_surfaces,
            terrain_texture_sets: project.terrain_texture_sets,
            terrain_texture_layers: project.terrain_texture_layers,
            terrain_profiles: project.terrain_profiles,
            assets,
            definitions,
            dependencies: self.dependencies,
            definition_dependencies: self.definition_dependencies,
            terrain_surface_dependencies: self.terrain_surface_dependencies,
        }
    }
}

fn page_key(source_cell: &SourceCellRecord, domain: PageDomain) -> PageKey {
    PageKey {
        space: source_cell.space,
        cell: source_cell.cell,
        domain,
        lod: 0,
    }
}

/// Everything the pages depend on that is not itself a page, in a fixed order.
fn hash_sources(
    hasher: &mut blake3::Hasher,
    project: &ProjectDocument,
    environment: &CookedEnvironment,
) -> Result<()> {
    hasher.update(&RUNTIME_SCHEMA_VERSION.to_le_bytes());

    hasher.update(&project.default_world_space.0.to_le_bytes());
    for space in &project.world_spaces {
        hasher.update(&space.id.0.to_le_bytes());
        hasher.update(space.name.as_bytes());
        hasher.update(&space.cell_size.to_bits().to_le_bytes());
        hasher.update(&space.minimum_y.to_bits().to_le_bytes());
        hasher.update(&space.maximum_y.to_bits().to_le_bytes());
        space.atmosphere.validate().map_err(anyhow::Error::msg)?;
        hasher.update(&bincode::serde::encode_to_vec(
            &space.atmosphere,
            bincode::config::standard(),
        )?);
    }
    hash_terrain_catalog(hasher, project, environment);
    hasher.update(&environment.fingerprint);
    if let Some(catalog) = &environment.catalog {
        let encoded = bincode::serde::encode_to_vec(
            catalog,
            bincode::config::standard()
                .with_little_endian()
                .with_fixed_int_encoding(),
        )?;
        hasher.update(&encoded);
    }
    for heightfield in &project.terrain_cell_heightfields {
        hasher.update(&heightfield.space.0.to_le_bytes());
        hasher.update(&heightfield.cell.x.to_le_bytes());
        hasher.update(&heightfield.cell.z.to_le_bytes());
        hasher.update(&heightfield.resolution.to_le_bytes());
        for height in &heightfield.heights {
            hasher.update(&height.to_bits().to_le_bytes());
        }
        hasher.update(&heightfield.source_revision.to_le_bytes());
    }
    for asset in &project.assets {
        hasher.update(&asset.id.0);
        hasher.update(asset.key.as_bytes());
        hasher.update(asset.kind.as_bytes());
        hasher.update(asset.source_uri.as_bytes());
    }
    for variant in &project.asset_variants {
        hasher.update(&variant.asset.0);
        hasher.update(&[variant.lod]);
        hasher.update(variant.uri.as_bytes());
        for bound in variant.bounds {
            hasher.update(&bound.to_bits().to_le_bytes());
        }
        hasher.update(&variant.gpu_bytes_estimate.to_le_bytes());
        hasher.update(&variant.shadow_policy.to_le_bytes());
        hasher.update(&variant.minimum_screen_height.to_bits().to_le_bytes());
    }
    for definition in &project.definitions {
        hasher.update(&definition.id.0);
        hasher.update(definition.key.as_bytes());
        hasher.update(definition.display_name.as_bytes());
        hasher.update(&[definition.activation as u8]);
        if let Some(asset) = definition.visual_asset {
            hasher.update(&asset.0);
        }
    }
    Ok(())
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
        || profile.composite_minimum_level > MAX_TERRAIN_NODE_LEVEL
    {
        bail!("terrain profile for {:?} is invalid", profile.space);
    }
    Ok(())
}

fn validate_terrain_texture_set(texture_set: &TerrainTextureSet) -> Result<()> {
    if texture_set.key.is_empty()
        || [
            &texture_set.base_color_universal_uri,
            &texture_set.normal_material_universal_uri,
            &texture_set.macro_variation_universal_uri,
            &texture_set.base_color_astc_uri,
            &texture_set.normal_material_astc_uri,
            &texture_set.macro_variation_astc_uri,
        ]
        .into_iter()
        .any(|uri| uri.is_empty())
    {
        bail!("terrain texture set {:?} is invalid", texture_set.id);
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum WeightEdge {
    Left,
    Right,
    Bottom,
    Top,
}

fn validate_terrain_heightfield_borders(
    pages_by_cell: &BTreeMap<(WorldSpaceId, CellCoord), &SourceTerrainCellHeightfieldRecord>,
) -> Result<()> {
    for (&(space, cell), page) in pages_by_cell {
        for &(dx, dz, current_edge, neighbour_edge) in &[
            (1, 0, WeightEdge::Right, WeightEdge::Left),
            (0, 1, WeightEdge::Top, WeightEdge::Bottom),
        ] {
            let neighbour_cell = CellCoord {
                x: cell.x + dx,
                z: cell.z + dz,
            };
            let Some(neighbour) = pages_by_cell.get(&(space, neighbour_cell)) else {
                continue;
            };
            if page.resolution != neighbour.resolution
                || height_edge(page, current_edge)
                    .iter()
                    .zip(height_edge(neighbour, neighbour_edge))
                    .any(|(left, right)| (*left - right).abs() > 1.0e-5)
            {
                bail!(
                    "terrain heightfield borders do not match between {:?} and {:?}",
                    cell,
                    neighbour_cell
                );
            }
        }
    }
    Ok(())
}

fn height_edge(page: &SourceTerrainCellHeightfieldRecord, edge: WeightEdge) -> Vec<f32> {
    let resolution = usize::from(page.resolution);
    (0..resolution)
        .map(|index| {
            let sample = match edge {
                WeightEdge::Left => index * resolution,
                WeightEdge::Right => index * resolution + resolution - 1,
                WeightEdge::Bottom => index,
                WeightEdge::Top => (resolution - 1) * resolution + index,
            };
            page.heights[sample]
        })
        .collect()
}

fn hash_terrain_catalog(
    hasher: &mut blake3::Hasher,
    project: &ProjectDocument,
    environment: &CookedEnvironment,
) {
    for surface in &project.terrain_surfaces {
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
        ] {
            hasher.update(&value.to_bits().to_le_bytes());
        }
    }
    for texture_set in &project.terrain_texture_sets {
        hasher.update(&texture_set.id.0);
        hasher.update(texture_set.key.as_bytes());
        for uri in [
            &texture_set.base_color_universal_uri,
            &texture_set.normal_material_universal_uri,
            &texture_set.macro_variation_universal_uri,
            &texture_set.base_color_astc_uri,
            &texture_set.normal_material_astc_uri,
            &texture_set.macro_variation_astc_uri,
        ] {
            hasher.update(uri.as_bytes());
        }
        hasher.update(&texture_set.universal_gpu_bytes.to_le_bytes());
        hasher.update(&texture_set.astc_gpu_bytes.to_le_bytes());
    }
    for layer in &project.terrain_texture_layers {
        hasher.update(&layer.texture_set.0);
        hasher.update(&layer.surface.0);
        hasher.update(&layer.layer.to_le_bytes());
    }
    for profile in &project.terrain_profiles {
        hasher.update(&profile.space.0.to_le_bytes());
        hasher.update(&profile.texture_set.0);
        hasher.update(&profile.weight_resolution.to_le_bytes());
        for value in profile.macro_scales {
            hasher.update(&value.to_bits().to_le_bytes());
        }
        hasher.update(&profile.macro_contrast.to_bits().to_le_bytes());
        hasher.update(&profile.macro_albedo_strength.to_bits().to_le_bytes());
        hasher.update(&[profile.composite_minimum_level]);
    }
    for slot in &environment.slots {
        hasher.update(&slot.space.0.to_le_bytes());
        hasher.update(&slot.cell.x.to_le_bytes());
        hasher.update(&slot.cell.z.to_le_bytes());
        hasher.update(&[slot.slot]);
        hasher.update(&slot.surface.0);
    }
    for weights in &environment.weights {
        hasher.update(&weights.space.0.to_le_bytes());
        hasher.update(&weights.cell.x.to_le_bytes());
        hasher.update(&weights.cell.z.to_le_bytes());
        hasher.update(&[weights.page]);
        hasher.update(&weights.resolution.to_le_bytes());
        hasher.update(&weights.rgba);
        hasher.update(&weights.source_revision.to_le_bytes());
    }
}

fn encoded_page(
    key: PageKey,
    payload: PagePayload,
    gpu_bytes_estimate: u64,
) -> Result<EncodedPage> {
    let decoded = encode_page_payload(&payload)?;
    let checksum = *blake3::hash(&decoded).as_bytes();
    let encoded = zstd::stream::encode_all(decoded.as_slice(), 1)?;
    Ok(EncodedPage {
        key,
        codec: PageCodec::Zstd,
        decoded_bytes: decoded.len() as u64,
        gpu_bytes_estimate,
        checksum,
        payload: encoded,
    })
}

fn estimate_heightfield_gpu_bytes(resolution: u16) -> u64 {
    let resolution = u64::from(resolution);
    let vertex_bytes = resolution * resolution * 48;
    let index_bytes = (resolution - 1) * (resolution - 1) * 6 * size_of::<u32>() as u64;
    vertex_bytes + index_bytes
}

fn hash_page(hasher: &mut blake3::Hasher, page: &EncodedPage) {
    hasher.update(&page.key.space.0.to_le_bytes());
    hasher.update(&page.key.cell.x.to_le_bytes());
    hasher.update(&page.key.cell.z.to_le_bytes());
    hasher.update(&(page.key.domain as i64).to_le_bytes());
    hasher.update(&[page.key.lod]);
    hasher.update(&page.checksum);
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
            composite_minimum_level: MAX_TERRAIN_NODE_LEVEL,
        };
        assert!(validate_terrain_profile(&profile).is_ok());

        profile.weight_resolution = MAX_TERRAIN_WEIGHT_RESOLUTION + 1;
        assert!(validate_terrain_profile(&profile).is_err());

        profile.weight_resolution = MAX_TERRAIN_WEIGHT_RESOLUTION;
        profile.macro_albedo_strength = 0.500_1;
        assert!(validate_terrain_profile(&profile).is_err());

        profile.macro_albedo_strength = 0.5;
        profile.composite_minimum_level = MAX_TERRAIN_NODE_LEVEL + 1;
        assert!(validate_terrain_profile(&profile).is_err());
    }
}
