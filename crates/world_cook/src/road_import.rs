//! Road styles and routes from RON (`yarra-world-cook import-roads`), for worlds whose roads are
//! laid out by tools rather than in the editor (`content/roads/`). Each style becomes a cart-track
//! style with its own Ground preset; each route a road whose knots are its points, joined by
//! smooth cubic spans. Identities derive from names, so importing the file again replaces what
//! it imported before and leaves other roads alone. The editor edits imported roads as any other.
use crate::terrain_pack::TerrainPack;
use anyhow::{Context, Result, bail};
use environment::{
    GRASS_CHANNEL, GroundTreatment, OutputId, Preset, PresetId, PresetKind, SurfaceWeight,
    roads::{
        CartTrackProfile, Road, RoadId, RoadKnot, RoadKnotId, RoadPoint, RoadProfileId, RoadRelief,
        RoadSpanId, RoadTravelMode, TravelDirection,
    },
};
use serde::Deserialize;
use std::path::Path;
use world::CellCoord;
use world_db::{
    RoadRecordKey, RoadSourceRecord, RoadSourceWrite, RoadSourceWriteResult, SourceRoad,
    SourceRoadKnot, SourceRoadSpan,
};

/// The most knots one route may have: its records must fit one road transaction.
const MAX_ROUTE_KNOTS: usize = 30;

#[derive(Deserialize)]
struct RoadImport {
    styles: Vec<StyleImport>,
    roads: Vec<RouteImport>,
}

/// A cart-track style: `CartTrackProfile`'s parameters, and the terrain surfaces (keys of the
/// world's terrain pack, with weights) its Ground preset lays down.
#[derive(Deserialize)]
struct StyleImport {
    name: String,
    surfaces: Vec<(String, f32)>,
    track_spacing: f32,
    track_width: f32,
    edge_softness: f32,
    center_ground: f32,
    center_retention: f32,
    shoulder_ground: f32,
    shoulder_retention: f32,
    track_retention: f32,
    edge_variation: f32,
    edge_patch_size: f32,
    breakup: f32,
    breakup_patch_size: f32,
    relief: RoadRelief,
}

/// A road through `points` (world XZ, metres), `width` metres across.
#[derive(Deserialize)]
struct RouteImport {
    name: String,
    style: String,
    width: f32,
    seed: u32,
    points: Vec<[f64; 2]>,
}

pub struct RoadImportReport {
    pub styles: usize,
    pub roads: usize,
    pub knots: usize,
}

pub fn import_roads(text: &str, project: &Path) -> Result<RoadImportReport> {
    let import: RoadImport = ron::from_str(text).context("road import RON")?;
    let reader = world_db::ProjectReader::open_read_only(project)?;
    let space_id = reader.manifest().default_world_space;
    let space = reader
        .manifest()
        .world_space(space_id)
        .context("project has no default world space")?
        .clone();
    let (_, mut library, _) = reader.read_environment_catalog()?;
    let terrain = TerrainPack::baltic();
    let mut profiles = Vec::new();
    for style in &import.styles {
        let ground = PresetId(crate::stable_id(&format!(
            "road-style/{}/ground",
            style.name
        )));
        let surfaces = style
            .surfaces
            .iter()
            .map(|(key, weight)| {
                terrain
                    .surfaces
                    .iter()
                    .find(|s| &s.key == key)
                    .map(|s| SurfaceWeight {
                        surface: s.id,
                        weight: *weight,
                    })
                    .with_context(|| format!("style {}: no terrain surface {key}", style.name))
            })
            .collect::<Result<Vec<_>>>()?;
        let preset = Preset {
            id: ground,
            revision: 1,
            name: format!("{} ground", style.name),
            kind: PresetKind::Ground(GroundTreatment {
                id: OutputId(crate::stable_id(&format!(
                    "road-style/{}/output",
                    style.name
                ))),
                strength: 1.0,
                surfaces,
            }),
        };
        match library.presets.iter_mut().find(|p| p.id == ground) {
            Some(existing) => {
                *existing = Preset {
                    revision: existing.revision + 1,
                    ..preset
                }
            }
            None => library.presets.push(preset),
        }
        profiles.push(CartTrackProfile {
            id: profile_id(&style.name),
            revision: 1,
            name: style.name.clone(),
            ground,
            vegetation_channel: GRASS_CHANNEL,
            track_spacing: style.track_spacing,
            track_width: style.track_width,
            edge_softness: style.edge_softness,
            center_ground: style.center_ground,
            center_retention: style.center_retention,
            shoulder_ground: style.shoulder_ground,
            shoulder_retention: style.shoulder_retention,
            track_retention: style.track_retention,
            edge_variation: style.edge_variation,
            edge_patch_size: style.edge_patch_size,
            breakup: style.breakup,
            breakup_patch_size: style.breakup_patch_size,
            relief: style.relief.clone(),
        });
    }
    drop(reader);

    // Styles and their ground presets first, in one transaction.
    let mut writer = world_db::ProjectWriter::open(project)?;
    let keys: Vec<_> = profiles
        .iter()
        .map(|p| RoadRecordKey::Profile(p.id))
        .collect();
    let states = writer.read_road_records(&keys)?;
    let writes: Vec<_> = profiles
        .into_iter()
        .zip(states)
        .map(|(profile, state)| RoadSourceWrite {
            key: RoadRecordKey::Profile(profile.id),
            expected_revision: state.revision,
            record: Some(RoadSourceRecord::Profile(profile)),
        })
        .collect();
    let revision = library.revision;
    let committed = writer.apply_environment_and_roads_transaction(
        revision,
        Some(&library),
        &[],
        &[],
        &writes,
        &[],
    )?;
    if !matches!(
        committed,
        world_db::EnvironmentSourceWriteResult::Committed(_)
    ) {
        bail!("road styles were not saved: {committed:?}");
    }

    // Then each road on its own, replacing any knots and spans a longer earlier import left.
    let mut knots_total = 0;
    for route in &import.roads {
        if !(2..=MAX_ROUTE_KNOTS).contains(&route.points.len()) {
            bail!(
                "road {}: {} points; a road takes 2 to {MAX_ROUTE_KNOTS}",
                route.name,
                route.points.len()
            );
        }
        if !import.styles.iter().any(|s| s.name == route.style) {
            bail!("road {}: no style {}", route.name, route.style);
        }
        let road = RoadId(crate::stable_id(&format!("road/{}", route.name)));
        let knot_id =
            |i: usize| RoadKnotId(crate::stable_id(&format!("road/{}/knot/{i}", route.name)));
        let span_id =
            |i: usize| RoadSpanId(crate::stable_id(&format!("road/{}/span/{i}", route.name)));
        let knots = knots(route, space.cell_size, knot_id);
        let mut records = vec![RoadSourceRecord::Road(SourceRoad {
            space: space_id,
            road: Road {
                id: road,
                revision: 1,
                name: route.name.clone(),
                profile: profile_id(&route.style),
                seed: route.seed,
                enabled: true,
                order: 0,
                direction: TravelDirection::Bidirectional,
                travel_modes: vec![
                    RoadTravelMode::Foot,
                    RoadTravelMode::Mounted,
                    RoadTravelMode::Cart,
                ],
            },
        })];
        records.extend(knots.iter().map(|k| {
            RoadSourceRecord::Knot(SourceRoadKnot {
                road,
                knot: k.clone(),
            })
        }));
        records.extend(knots.windows(2).enumerate().map(|(i, pair)| {
            RoadSourceRecord::Span(SourceRoadSpan {
                id: span_id(i),
                revision: 1,
                road,
                start: pair[0].id,
                end: pair[1].id,
            })
        }));
        // Stale knots and spans past this import's end are removed with it.
        let mut keys: Vec<_> = records.iter().map(RoadSourceRecord::key).collect();
        let stale: Vec<_> = (knots.len()..MAX_ROUTE_KNOTS)
            .map(|i| RoadRecordKey::Knot(knot_id(i)))
            .chain((knots.len() - 1..MAX_ROUTE_KNOTS).map(|i| RoadRecordKey::Span(span_id(i))))
            .collect();
        keys.extend(&stale);
        let states = writer.read_road_records(&keys)?;
        let mut writes: Vec<_> = records
            .into_iter()
            .zip(&states)
            .map(|(record, state)| RoadSourceWrite {
                key: record.key(),
                expected_revision: state.revision,
                record: Some(record),
            })
            .collect();
        writes.extend(
            states[writes.len()..]
                .iter()
                .filter(|s| s.record.is_some())
                .map(|s| RoadSourceWrite {
                    key: s.key,
                    expected_revision: s.revision,
                    record: None,
                }),
        );
        // The style the road is read with is checked against its current revision.
        let style = RoadRecordKey::Profile(profile_id(&route.style));
        let dependencies: Vec<_> = writer
            .read_road_records(&[style])?
            .into_iter()
            .filter_map(|s| {
                Some(world_db::RoadDependency {
                    key: s.key,
                    revision: s.revision?,
                })
            })
            .collect();
        let library_revision = writer.road_library_revision()?;
        match writer.apply_road_source_transaction(library_revision, &writes, &dependencies)? {
            RoadSourceWriteResult::Committed(_) => {}
            other => bail!("road {} was not saved: {other:?}", route.name),
        }
        knots_total += knots.len();
    }
    Ok(RoadImportReport {
        styles: import.styles.len(),
        roads: import.roads.len(),
        knots: knots_total,
    })
}

fn profile_id(style: &str) -> RoadProfileId {
    RoadProfileId(crate::stable_id(&format!("road-style/{style}")))
}

/// Knots at the route's points with smooth handles: each along the direction from the previous
/// point to the next, a third of the way to its neighbour on that side.
fn knots(route: &RouteImport, cell_size: f32, id: impl Fn(usize) -> RoadKnotId) -> Vec<RoadKnot> {
    let p = &route.points;
    let size = f64::from(cell_size);
    (0..p.len())
        .map(|i| {
            let previous = p[i.saturating_sub(1)];
            let next = p[(i + 1).min(p.len() - 1)];
            let direction = [next[0] - previous[0], next[1] - previous[1]];
            let length = direction[0].hypot(direction[1]).max(1e-9);
            let unit = [direction[0] / length, direction[1] / length];
            let reach = |other: [f64; 2]| (other[0] - p[i][0]).hypot(other[1] - p[i][1]) / 3.0;
            let cell = CellCoord {
                x: (p[i][0] / size).floor() as i32,
                z: (p[i][1] / size).floor() as i32,
            };
            let local = [
                p[i][0] - f64::from(cell.x) * size,
                p[i][1] - f64::from(cell.z) * size,
            ];
            let incoming = if i == 0 { 0.0 } else { reach(previous) };
            let outgoing = if i + 1 == p.len() { 0.0 } else { reach(next) };
            RoadKnot {
                id: id(i),
                revision: 1,
                position: RoadPoint { cell, local },
                incoming: [-unit[0] * incoming, -unit[1] * incoming],
                outgoing: [unit[0] * outgoing, unit[1] * outgoing],
                width: route.width,
            }
        })
        .collect()
}
