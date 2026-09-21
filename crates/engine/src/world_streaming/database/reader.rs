//! Blocking immutable-reader work only. No ECS, renderer, or residency dependencies.
use super::protocol::*;
use crossbeam_channel::{Receiver, Sender};
use std::{collections::BTreeMap, path::PathBuf};
use world::PageDomain;
use world_db::RuntimeReader;

// Cancellation is out of band so teardown can release a worker blocked on either channel.
fn reply(
    results: &Sender<DatabaseResult>,
    cancel: &Receiver<()>,
    result: DatabaseResult,
) -> Result<(), ()> {
    crossbeam_channel::select_biased! {
        recv(cancel) -> _ => Err(()),
        send(results, result) -> sent => sent.map_err(|_| ()),
    }
}

pub(super) fn run(
    path: PathBuf,
    requests: Receiver<DatabaseRequest>,
    results: Sender<DatabaseResult>,
    cancel: Receiver<()>,
) {
    let mut reader = match RuntimeReader::open_immutable(&path) {
        Ok(reader) => {
            if reply(
                &results,
                &cancel,
                DatabaseResult::Opened(Ok(reader.manifest().clone())),
            )
            .is_err()
            {
                return;
            }
            reader
        }
        Err(error) => {
            let _ = reply(
                &results,
                &cancel,
                DatabaseResult::Opened(Err(format!("could not open {}: {error}", path.display()))),
            );
            return;
        }
    };

    let mut candidate: Option<(u64, RuntimeReader)> = None;
    loop {
        let request = crossbeam_channel::select_biased! {
            recv(cancel) -> _ => return,
            recv(requests) -> request => match request {
                Ok(request) => request,
                Err(_) => return,
            },
        };
        match request {
            DatabaseRequest::Terrain {
                request_id,
                generation,
                query,
            } => {
                let result = if reader.manifest().generation_id == generation {
                    read_terrain(&reader, query)
                } else if let Some((_, staged)) = &candidate
                    && staged.manifest().generation_id == generation
                {
                    read_terrain(staged, query)
                } else {
                    Err("terrain request belongs to a stale generation".into())
                };
                if reply(
                    &results,
                    &cancel,
                    DatabaseResult::Terrain { request_id, result },
                )
                .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::Reload {
                request_id,
                expected_generation,
            } => {
                let result = RuntimeReader::open_immutable(&path)
                    .map_err(|error| format!("could not reopen {}: {error}", path.display()))
                    .and_then(|opened| {
                        let manifest = opened.manifest().clone();
                        if manifest.generation_id != expected_generation {
                            return Err(format!(
                                "published generation mismatch: expected {expected_generation}, opened {}",
                                manifest.generation_id
                            ));
                        }
                        candidate = Some((request_id, opened));
                        Ok(manifest)
                    });
                if reply(
                    &results,
                    &cancel,
                    DatabaseResult::Reloaded { request_id, result },
                )
                .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::CommitReload {
                request_id,
                expected_generation,
            } => {
                let result = if candidate.as_ref().is_some_and(|(id, r)| {
                    *id == request_id && r.manifest().generation_id == expected_generation
                }) {
                    reader = candidate.take().unwrap().1;
                    Ok(())
                } else {
                    Err("no matching prepared database generation to commit".into())
                };
                if reply(
                    &results,
                    &cancel,
                    DatabaseResult::ReloadCommitted { request_id, result },
                )
                .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::DiscardReload { request_id } => {
                if candidate.as_ref().is_some_and(|(id, _)| *id == request_id) {
                    candidate = None;
                }
            }
            DatabaseRequest::ReadIndex {
                generation,
                revision,
                space,
                windows,
            } => {
                let result = (if reader.manifest().generation_id == generation {
                    Ok(&reader)
                } else {
                    Err("source index belongs to a stale generation".to_string())
                })
                .and_then(|reader| {
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
                });
                if reply(
                    &results,
                    &cancel,
                    DatabaseResult::Index {
                        revision,
                        space,
                        result,
                    },
                )
                .is_err()
                {
                    return;
                }
            }
            DatabaseRequest::ReadPage {
                generation,
                request_id,
                key,
                height_only,
            } => {
                let result = (if reader.manifest().generation_id == generation {
                    Ok(&reader)
                } else {
                    Err("source page belongs to a stale generation".to_string())
                })
                .and_then(|reader| {
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
                                let terrain = if key.domain == PageDomain::TerrainRender {
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
                });
                if reply(
                    &results,
                    &cancel,
                    DatabaseResult::Page {
                        request_id,
                        key,
                        result,
                    },
                )
                .is_err()
                {
                    return;
                }
            }
        }
    }
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
            .has_terrain_composites(space)
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
