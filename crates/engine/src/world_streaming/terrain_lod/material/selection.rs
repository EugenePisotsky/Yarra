//! Texture demand uses rotation-independent texel size and 3D bounds, never geometry error.
use super::*;

pub(super) struct Plan {
    pub keys: Vec<TerrainMaterialKey>,
    pub metadata: BTreeSet<TerrainMaterialKey>,
    pub limited: bool,
}
fn bounds(d: &TerrainCompositeDescriptor, size: f64) -> [DVec3; 2] {
    let extent = (1_u64 << d.key.0.level) as f64 * size;
    let x = d.key.0.x as f64 * extent;
    let z = d.key.0.z as f64 * extent;
    [
        DVec3::new(x, d.height_bounds[0] as f64, z),
        DVec3::new(x + extent, d.height_bounds[1] as f64, z + extent),
    ]
}
fn score(d: &TerrainCompositeDescriptor, view: &LodView, size: f64) -> f64 {
    let texel = size * (1_u64 << d.key.0.level) as f64 / 64.;
    view.distance_error(bounds(d, size), texel as f32)
}

pub(super) fn plan(
    roots: &[TerrainNodeKey],
    descriptors: &BTreeMap<TerrainMaterialKey, TerrainCompositeDescriptor>,
    previous: &BTreeSet<TerrainMaterialKey>,
    view: &LodView,
    size: f64,
    capacity: usize,
    minimum_level: u8,
) -> Plan {
    let mut output = Plan {
        keys: vec![],
        metadata: BTreeSet::new(),
        limited: false,
    };
    let mut frontier: Vec<_> = roots
        .iter()
        .filter_map(|k| descriptors.get(&TerrainMaterialKey(*k)))
        .map(|d| (score(d, view, size), d.key))
        .collect();
    while let Some((i, _)) = frontier
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.0.total_cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
    {
        let (pixels, key) = frontier.swap_remove(i);
        if key.0.level <= minimum_level {
            continue;
        }
        let Some(children) = key.0.children().ok().flatten() else {
            continue;
        };
        let retained = children
            .iter()
            .any(|k| previous.contains(&TerrainMaterialKey(*k)));
        if pixels <= if retained { 1.25 } else { 2.0 } {
            continue;
        }
        if output.keys.len() + 4 > capacity {
            output.limited = true;
            continue;
        }
        if children
            .iter()
            .any(|k| !descriptors.contains_key(&TerrainMaterialKey(*k)))
        {
            output.metadata.extend(
                children
                    .into_iter()
                    .map(TerrainMaterialKey)
                    .filter(|k| !descriptors.contains_key(k)),
            );
            // No more than a single bounded descriptor batch per planning iteration.
            if output.metadata.len() >= 128 {
                break;
            }
            continue;
        }
        for child in children {
            let key = TerrainMaterialKey(child);
            let pixels = score(&descriptors[&key], view, size);
            if pixels > 0. {
                output.keys.push(key);
                frontier.push((pixels, key));
            }
        }
    }
    output
}
