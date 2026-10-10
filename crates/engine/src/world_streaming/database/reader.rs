//! Blocking immutable-reader work only. No ECS, renderer, or residency dependencies.
use super::protocol::*;
use crossbeam_channel::{Receiver, Sender};
use std::{collections::BTreeMap, path::PathBuf};
use world::{PageDomain, PageKey};
use world_db::RuntimeReader;

// Cancellation is out of band so teardown can release a worker blocked on either channel.
fn reply(
    results: &Sender<(RequestId, DatabaseResult)>,
    cancel: &Receiver<()>,
    id: RequestId,
    result: DatabaseResult,
) -> Result<(), ()> {
    crossbeam_channel::select_biased! {
        recv(cancel) -> _ => Err(()),
        send(results, (id, result)) -> sent => sent.map_err(|_| ()),
    }
}

pub(super) fn run(
    path: PathBuf,
    requests: Receiver<(RequestId, DatabaseRequest)>,
    results: Sender<(RequestId, DatabaseResult)>,
    cancel: Receiver<()>,
) {
    let mut reader = match RuntimeReader::open_immutable(&path) {
        Ok(reader) => {
            let opened = DatabaseResult::Opened(Ok(reader.manifest().clone()));
            if reply(&results, &cancel, 0, opened).is_err() {
                return;
            }
            reader
        }
        Err(error) => {
            let error = format!("could not open {}: {error}", path.display());
            let _ = reply(&results, &cancel, 0, DatabaseResult::Opened(Err(error)));
            return;
        }
    };

    // The prepared candidate, named by the id of the reload request that opened it.
    let mut candidate: Option<(RequestId, RuntimeReader)> = None;
    loop {
        let (id, request) = crossbeam_channel::select_biased! {
            recv(cancel) -> _ => return,
            recv(requests) -> request => match request {
                Ok(request) => request,
                Err(_) => return,
            },
        };
        let live = |generation: &str, stale: &str| {
            if reader.manifest().generation_id == generation {
                Ok(&reader)
            } else {
                Err(stale.to_string())
            }
        };
        let result = match request {
            DatabaseRequest::Terrain { generation, query } => {
                DatabaseResult::Terrain(if reader.manifest().generation_id == generation {
                    read_terrain(&reader, query)
                } else if let Some((_, staged)) = &candidate
                    && staged.manifest().generation_id == generation
                {
                    read_terrain(staged, query)
                } else {
                    Err("terrain request belongs to a stale generation".into())
                })
            }
            DatabaseRequest::Reload {
                expected_generation,
            } => DatabaseResult::Reloaded(
                RuntimeReader::open_immutable(&path)
                    .map_err(|error| format!("could not reopen {}: {error}", path.display()))
                    .and_then(|opened| {
                        let manifest = opened.manifest().clone();
                        if manifest.generation_id != expected_generation {
                            return Err(format!(
                                "published generation mismatch: expected {expected_generation}, opened {}",
                                manifest.generation_id
                            ));
                        }
                        candidate = Some((id, opened));
                        Ok(manifest)
                    }),
            ),
            DatabaseRequest::CommitReload {
                reload,
                expected_generation,
            } => DatabaseResult::ReloadCommitted(
                if candidate.as_ref().is_some_and(|(id, r)| {
                    *id == reload && r.manifest().generation_id == expected_generation
                }) {
                    reader = candidate.take().unwrap().1;
                    Ok(())
                } else {
                    Err("no matching prepared database generation to commit".into())
                },
            ),
            DatabaseRequest::DiscardReload { reload } => {
                if candidate.as_ref().is_some_and(|(id, _)| *id == reload) {
                    candidate = None;
                }
                continue;
            }
            DatabaseRequest::ReadIndex {
                generation,
                space,
                windows,
            } => DatabaseResult::Index(live(&generation, "source index belongs to a stale generation").and_then(|reader| {
                windows
                    .into_iter()
                    .try_fold(BTreeMap::new(), |mut cells, [min, max]| {
                        for d in reader.read_cell_descriptors(space, min, max)? {
                            cells.insert(d.cell, d);
                        }
                        Ok::<_, world_db::WorldDbError>(cells)
                    })
                    .map(|cells| cells.into_values().collect())
                    .map_err(|error| error.to_string())
            })),
            DatabaseRequest::ReadPage {
                generation,
                key,
                height_only,
            } => DatabaseResult::Page {
                key,
                result: live(&generation, "source page belongs to a stale generation")
                    .and_then(|reader| read_page(reader, key, height_only)),
            },
            DatabaseRequest::ReadFarObjects {
                generation,
                space,
                blocks,
            } => DatabaseResult::FarObjects(live(&generation, "far objects belong to a stale generation").and_then(|reader| {
                blocks
                    .into_iter()
                    .map(|block| {
                        let page = reader.read_far_objects(space, block, block)?;
                        Ok((block, page.into_iter().next().map(|(_, payload)| payload)))
                    })
                    .collect::<Result<Vec<_>, world_db::WorldDbError>>()
                    .map_err(|error| error.to_string())
            })),
        };
        if reply(&results, &cancel, id, result).is_err() {
            return;
        }
    }
}

fn read_page(
    reader: &RuntimeReader,
    key: PageKey,
    height_only: bool,
) -> Result<Option<FetchedPage>, String> {
    reader
        .read_page(key)
        .and_then(|page| {
            page.map(|page| {
                let dependencies = if height_only {
                    Vec::new()
                } else {
                    reader.read_dependencies(key)?
                };
                let definitions = if key.domain == PageDomain::GameplayObjects {
                    reader.read_object_definitions(key)?
                } else {
                    Vec::new()
                };
                let terrain = if key.domain == PageDomain::Terrain {
                    Some(reader.read_terrain_resources(key)?)
                } else {
                    None
                };
                Ok(FetchedPage {
                    encoded: page,
                    dependencies,
                    definitions,
                    terrain,
                    height_only,
                })
            })
            .transpose()
        })
        .map_err(|error| error.to_string())
}

fn read_terrain(reader: &RuntimeReader, query: TerrainQuery) -> Result<TerrainReply, String> {
    match query {
        TerrainQuery::Material(query) => read_material(reader, query).map(TerrainReply::Material),
        TerrainQuery::Roots(space) => reader
            .read_terrain_roots(space)
            .map(TerrainReply::Metadata)
            .map_err(|e| e.to_string()),
        TerrainQuery::Metadata(keys) => reader
            .read_terrain_node_descriptors(&keys)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map(TerrainReply::Metadata)
            .ok_or("missing required terrain metadata".into()),
        TerrainQuery::Node(key) => reader
            .read_terrain_node(key)
            .map_err(|e| e.to_string())?
            .map(TerrainReply::Node)
            .ok_or("missing required terrain node".into()),
    }
}

fn read_material(
    reader: &RuntimeReader,
    query: TerrainMaterialQuery,
) -> Result<TerrainMaterialReply, String> {
    match query {
        TerrainMaterialQuery::Presence(space) => reader
            .terrain_composite_minimum_level(space)
            .map(TerrainMaterialReply::Presence)
            .map_err(|e| e.to_string()),
        TerrainMaterialQuery::Descriptors(keys) => reader
            .read_terrain_composite_descriptors(&keys)
            .map_err(|e| e.to_string())?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .map(TerrainMaterialReply::Descriptors)
            .ok_or("missing declared terrain composite".into()),
        TerrainMaterialQuery::Tile(key) => reader
            .read_terrain_composite(key)
            .map_err(|e| e.to_string())?
            .map(TerrainMaterialReply::Tile)
            .ok_or("missing declared terrain composite".into()),
    }
}
