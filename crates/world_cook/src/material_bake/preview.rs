//! Regional, unpublished hierarchy rebuild. Never modifies the source/runtime DB.
use super::{
    TerrainBakeLibrary, evaluate,
    filter::{self, Core, N, Pixel},
};
use anyhow::{Context, Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use world::*;
use world_db::{RuntimeReader, TerrainRenderResources};

pub const MAX_PREVIEW_LEAVES: usize = 256;
const MAX_PREVIEW_BYTES: usize = 96 * 1024 * 1024;
const MAX_CORE_READS: usize = 4096;

pub fn published_terrain_leaf(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    cell: CellCoord,
) -> Result<Option<TerrainHeightfieldPage>> {
    let key = PageKey {
        space,
        cell,
        domain: PageDomain::TerrainRender,
        lod: 0,
    };
    let Some(page) = reader.read_page(key)? else {
        return Ok(None);
    };
    match page.decode()?.payload {
        PagePayload::TerrainHeightfield(page) => Ok(Some(page)),
        PagePayload::TerrainRender(page) => {
            let size = reader
                .manifest()
                .world_space(space)
                .context("missing terrain space")?
                .cell_size;
            Ok(Some(TerrainHeightfieldPage {
                heightfield: TerrainHeightfield::from_heights(
                    2,
                    &[page.height; 4],
                    page.height,
                    page.height,
                    size,
                )?,
                surfaces: page.surfaces,
                weight_pages: page.weight_pages,
            }))
        }
        _ => bail!("unexpected terrain leaf payload"),
    }
}

/// Complete sparse override relative to the immutable publication. Unchanged
/// siblings are loaded one at a time. Quantized published interiors are the
/// fallback filter inputs, never a previously filtered preview (no undo drift).
pub fn bake_terrain_preview(
    reader: &RuntimeReader,
    space: WorldSpaceId,
    leaves: BTreeMap<CellCoord, Arc<TerrainHeightfieldPage>>,
    resources: &TerrainRenderResources,
    library: &TerrainBakeLibrary,
    cancelled: impl Fn() -> bool,
) -> Result<TerrainPreviewProducts> {
    if leaves.len() > MAX_PREVIEW_LEAVES {
        bail!("Live terrain exceeds 256 changed cells; Save & Publish to continue.")
    }
    let size = reader
        .manifest()
        .world_spaces
        .iter()
        .find(|s| s.id == space)
        .context("missing terrain space")?
        .cell_size;
    let roots: BTreeSet<_> = reader
        .read_terrain_roots(space)?
        .into_iter()
        .map(|d| d.key)
        .collect();
    let mut result = TerrainPreviewProducts {
        leaves,
        ..Default::default()
    };
    let mut changed = BTreeSet::new();
    for (&cell, page) in &result.leaves {
        if cancelled() {
            bail!("superseded terrain preview")
        }
        let key = TerrainNodeKey::leaf(space, cell);
        let old = reader
            .read_terrain_node(key)?
            .context("preview cannot add terrain outside the published domain")?
            .decode()?;
        let node = TerrainNode::leaf(
            key,
            &page.heightfield,
            old.heightfield
                .as_ref()
                .context("missing leaf samples")?
                .resolution,
        )?;
        if node != old {
            changed.insert(key);
            result.nodes.insert(key, Arc::new(node));
        }
    }
    // Check borders even across separate coarse roots, where parent validation
    // alone would never see the mismatch. A missing relief halo fails closed.
    for &key in &changed {
        let h = result.nodes[&key].heightfield.as_ref().unwrap();
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let other = TerrainNodeKey {
                x: key.x + dx,
                z: key.z + dz,
                ..key
            };
            let node = match result.nodes.get(&other) {
                Some(n) => Some(n.clone()),
                None => reader
                    .read_terrain_node(other)?
                    .map(|e| e.decode().map(Arc::new))
                    .transpose()?,
            };
            if let Some(other) = node {
                let b = other
                    .heightfield
                    .as_ref()
                    .context("missing neighbor samples")?;
                let n = usize::from(h.resolution);
                for i in 0..n {
                    let (a_i, b_i) = match (dx, dz) {
                        (-1, 0) => (i * n, i * n + n - 1),
                        (1, 0) => (i * n + n - 1, i * n),
                        (0, -1) => (i, (n - 1) * n + i),
                        _ => ((n - 1) * n + i, i),
                    };
                    if h.resolution != b.resolution
                        || h.heights[a_i].to_bits() != b.heights[b_i].to_bits()
                        || h.normals_oct[a_i] != b.normals_oct[b_i]
                    {
                        bail!(
                            "Live terrain needs its neighboring relief/normal cells; previous preview retained"
                        )
                    }
                }
            }
        }
    }
    while !changed.is_empty() {
        let parents: BTreeSet<_> = changed
            .iter()
            .filter(|k| !roots.contains(k))
            .filter_map(|k| k.parent().ok().flatten())
            .collect();
        for &key in &parents {
            if cancelled() {
                bail!("superseded terrain preview")
            }
            let mut children: [Option<Arc<TerrainNode>>; 4] = Default::default();
            for (i, child) in key
                .children()?
                .context("missing children")?
                .into_iter()
                .enumerate()
            {
                children[i] = match result.nodes.get(&child) {
                    Some(n) => Some(n.clone()),
                    None => reader
                        .read_terrain_node(child)?
                        .map(|e| e.decode().map(Arc::new))
                        .transpose()?,
                };
            }
            let node = TerrainNode::parent(key, std::array::from_fn(|i| children[i].as_deref()))?;
            result.nodes.insert(key, Arc::new(node));
        }
        changed = parents;
    }
    if !reader.has_terrain_composites(space)? {
        bail!(
            "Live terrain needs baked ground materials; Save & Publish once before editing with terrain LOD"
        )
    }
    let mut cores = BTreeMap::new();
    let mut changed = BTreeSet::new();
    for (&cell, page) in &result.leaves {
        if cancelled() {
            bail!("superseded terrain preview")
        }
        let key = TerrainMaterialKey(TerrainNodeKey::leaf(space, cell));
        cores.insert(
            key,
            evaluate::leaf(
                key,
                size,
                page,
                resources,
                library.get(&resources.texture_set)?,
            )?,
        );
        changed.insert(key);
    }
    let mut reads = 0;
    let mut finish = BTreeSet::new();
    while !changed.is_empty() {
        for key in &changed {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    finish.insert(TerrainMaterialKey(TerrainNodeKey {
                        x: key.0.x + dx,
                        z: key.0.z + dz,
                        ..key.0
                    }));
                }
            }
        }
        let parents: BTreeSet<_> = changed
            .iter()
            .filter(|k| !roots.contains(&k.0))
            .filter_map(|k| k.0.parent().ok().flatten().map(TerrainMaterialKey))
            .collect();
        for &key in &parents {
            if cancelled() {
                bail!("superseded terrain preview")
            }
            let mut children = [None, None, None, None];
            for (i, child) in key
                .0
                .children()?
                .context("missing material children")?
                .into_iter()
                .enumerate()
            {
                let child = TerrainMaterialKey(child);
                children[i] = match cores.get(&child) {
                    Some(c) => Some(c.clone()),
                    None => published_core(reader, child, &mut reads)?,
                };
            }
            cores.insert(key, filter::parent(&children));
        }
        changed = parents;
        if cores.len() * N * N * std::mem::size_of::<Pixel>() + result.bytes() > MAX_PREVIEW_BYTES {
            bail!("Live terrain filter budget exceeded; Save & Publish to continue")
        }
    }
    for key in finish {
        if cancelled() {
            bail!("superseded terrain preview")
        }
        if reader.read_terrain_composite_descriptors(&[key])?[0].is_none() {
            continue;
        }
        let mut neighbors: [Option<Core>; 9] = Default::default();
        for (i, neighbor) in neighbors.iter_mut().enumerate() {
            let k = TerrainMaterialKey(TerrainNodeKey {
                x: key.0.x + i as i32 % 3 - 1,
                z: key.0.z + i as i32 / 3 - 1,
                ..key.0
            });
            *neighbor = match cores.get(&k) {
                Some(c) => Some(c.clone()),
                None => published_core(reader, k, &mut reads)?,
            };
        }
        result
            .composites
            .insert(key, Arc::new(filter::finish(key, &neighbors)?));
        if result.bytes() + cores.len() * N * N * std::mem::size_of::<Pixel>() > MAX_PREVIEW_BYTES {
            bail!("Live terrain product budget exceeded; Save & Publish to continue")
        }
    }
    Ok(result)
}

fn published_core(
    reader: &RuntimeReader,
    key: TerrainMaterialKey,
    reads: &mut usize,
) -> Result<Option<Core>> {
    *reads += 1;
    if *reads > MAX_CORE_READS {
        bail!("Live terrain filter work limit exceeded")
    }
    if let Some(tile) = reader.read_terrain_composite(key)? {
        let tile = tile.decode()?;
        let mip = &tile.mips[0];
        let width = TerrainComposite::mip_size(0);
        let pixels = (0..N * N)
            .map(|i| {
                let j =
                    ((i / N + TERRAIN_COMPOSITE_GUTTER) * width + i % N + TERRAIN_COMPOSITE_GUTTER)
                        * 4;
                let srgb = |v: u8| {
                    let v = v as f32 / 255.;
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                let x = mip.response[j] as f32 / 255. * 2. - 1.;
                let z = mip.response[j + 1] as f32 / 255. * 2. - 1.;
                let y = 1. - x.abs() - z.abs();
                let normal = if y < 0. {
                    [(1. - z.abs()).copysign(x), y, (1. - x.abs()).copysign(z)]
                } else {
                    [x, y, z]
                };
                Pixel {
                    color: [
                        srgb(mip.color[j]),
                        srgb(mip.color[j + 1]),
                        srgb(mip.color[j + 2]),
                    ],
                    normal: filter::normalize(normal),
                    roughness: mip.response[j + 2] as f32 / 255.,
                    ao: mip.response[j + 3] as f32 / 255.,
                    hollow: mip.color[j + 3] as f32 / 255.,
                    valid: true,
                }
            })
            .collect();
        return Ok(Some(Core {
            fingerprint: tile.fingerprint,
            pixels,
        }));
    }
    let Some(d) = reader
        .read_terrain_node_descriptors(&[key.0])?
        .pop()
        .flatten()
    else {
        return Ok(None);
    };
    if d.child_mask == 0 {
        return Ok(None);
    }
    let mut children = [None, None, None, None];
    for (i, k) in key
        .0
        .children()?
        .context("partial material children")?
        .into_iter()
        .enumerate()
    {
        if d.child_mask & (1 << i) != 0 {
            children[i] = published_core(reader, TerrainMaterialKey(k), reads)?
        }
    }
    Ok(Some(filter::parent(&children)))
}
