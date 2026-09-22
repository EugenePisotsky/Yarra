use crate::catalog::{
    VEGETATION_CATALOG_FORMAT_VERSION, encode_vegetation_catalog, read_vegetation_catalog,
};
use crate::project::query::source_object_from_row;
use crate::storage::ensure_schema_version;
use crate::{
    MAX_OBJECT_WRITES_PER_TRANSACTION, ObjectTransformWriteResult, ObjectWriteTransactionResult,
    SourceObjectTransform, SourceObjectWrite, SourceObjectWriteCommit,
    VegetationCatalogWriteResult, WorldDbError, environment_store,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::HashSet;
use std::path::Path;
use vegetation::VegetationCatalog;
use world::{PROJECT_SCHEMA_VERSION, StableObjectId};

/// Narrow transactional writer used by the editor's authoring worker.
///
/// Updates and deletes compare source revisions in SQLite. A conflict rolls back the whole batch;
/// placement creation is used by the inverse of a previously committed deletion.
pub struct ProjectWriter {
    pub(crate) connection: Connection,
}

impl ProjectWriter {
    pub fn open(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 1000;")?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        Ok(Self { connection })
    }

    /// Replaces the generation-global vegetation catalog in one transaction.
    ///
    /// This is intentionally separate from spatial environment-mask writes: profile authoring changes a
    /// small global source domain, while painting and placement tools mutate bounded page domains.
    pub fn replace_vegetation_catalog(
        &mut self,
        catalog: &VegetationCatalog,
    ) -> Result<(), WorldDbError> {
        let payload = encode_vegetation_catalog(catalog)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        environment_store::validate_catalog_dependencies(&transaction, catalog)?;
        transaction.execute(
            "INSERT INTO vegetation_catalog(singleton, format_version, payload) \
             VALUES (1, ?1, ?2) \
             ON CONFLICT(singleton) DO UPDATE SET \
                 format_version = excluded.format_version, payload = excluded.payload",
            params![VEGETATION_CATALOG_FORMAT_VERSION, payload],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Replaces the global vegetation catalog only when the persisted catalog still matches the
    /// editor baseline. This gives the singleton source domain the same optimistic-concurrency
    /// behavior as spatial object and dense-page transactions without coupling it to their
    /// per-record revision columns.
    pub fn replace_vegetation_catalog_if_matches(
        &mut self,
        expected: Option<&VegetationCatalog>,
        replacement: &VegetationCatalog,
    ) -> Result<VegetationCatalogWriteResult, WorldDbError> {
        replacement.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let actual = read_vegetation_catalog(&transaction)?;
        if actual.as_ref() != expected {
            return Ok(VegetationCatalogWriteResult::Conflict { actual });
        }

        environment_store::validate_catalog_dependencies(&transaction, replacement)?;
        let payload = encode_vegetation_catalog(replacement)?;
        transaction.execute(
            "INSERT INTO vegetation_catalog(singleton, format_version, payload) \
             VALUES (1, ?1, ?2) \
             ON CONFLICT(singleton) DO UPDATE SET \
                 format_version = excluded.format_version, payload = excluded.payload",
            params![VEGETATION_CATALOG_FORMAT_VERSION, payload],
        )?;
        transaction.commit()?;
        Ok(VegetationCatalogWriteResult::Committed)
    }

    pub fn update_object_transform(
        &mut self,
        object: StableObjectId,
        expected_source_revision: i64,
        transform: SourceObjectTransform,
    ) -> Result<ObjectTransformWriteResult, WorldDbError> {
        let result = self.apply_object_transaction(&[SourceObjectWrite::UpdateTransform {
            object,
            expected_source_revision,
            transform,
        }])?;
        match result {
            ObjectWriteTransactionResult::Committed(mut commits) => match commits.pop() {
                Some(SourceObjectWriteCommit::Updated(object)) => {
                    Ok(ObjectTransformWriteResult::Updated(object))
                }
                _ => Err(WorldDbError::InvalidObjectTransaction),
            },
            ObjectWriteTransactionResult::Conflict { actual, .. } => {
                Ok(ObjectTransformWriteResult::Conflict { actual })
            }
        }
    }

    pub fn apply_object_transaction(
        &mut self,
        writes: &[SourceObjectWrite],
    ) -> Result<ObjectWriteTransactionResult, WorldDbError> {
        if writes.is_empty() || writes.len() > MAX_OBJECT_WRITES_PER_TRANSACTION {
            return Err(WorldDbError::InvalidObjectTransaction);
        }
        let mut objects = HashSet::with_capacity(writes.len());
        if writes.iter().any(|write| !objects.insert(write.object())) {
            return Err(WorldDbError::InvalidObjectTransaction);
        }

        let transaction = self.connection.transaction()?;
        let mut commits = Vec::with_capacity(writes.len());
        for write in writes {
            let (object, updated) = match write {
                SourceObjectWrite::Create { object } => {
                    validate_object_transform(SourceObjectTransform::from(object))?;
                    let actual = transaction
                        .query_row(
                            "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, \
                                    definition_id, local_x, local_y, local_z, yaw, scale, \
                                    source_revision \
                             FROM object_placements WHERE object_id = ?1",
                            params![object.id.0.as_slice()],
                            source_object_from_row,
                        )
                        .optional()?;
                    if actual.is_some() {
                        transaction.rollback()?;
                        return Ok(ObjectWriteTransactionResult::Conflict {
                            object: object.id,
                            actual,
                        });
                    }
                    let source_revision = object.source_revision.saturating_add(1);
                    let updated = transaction.execute(
                        "INSERT INTO object_placements( \
                            object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                            local_x, local_y, local_z, yaw, scale, source_revision \
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            object.id.0.as_slice(),
                            object.space.0,
                            object.owner_cell.x,
                            object.owner_cell.z,
                            object.definition.0.as_slice(),
                            object.local_translation[0],
                            object.local_translation[1],
                            object.local_translation[2],
                            object.yaw,
                            object.scale,
                            source_revision,
                        ],
                    )?;
                    (object.id, updated)
                }
                SourceObjectWrite::UpdateTransform {
                    object,
                    expected_source_revision,
                    transform,
                } => {
                    validate_object_transform(*transform)?;
                    let updated = transaction.execute(
                        "UPDATE object_placements \
                         SET world_space_id = ?1, owner_cell_x = ?2, owner_cell_z = ?3, \
                             local_x = ?4, local_y = ?5, local_z = ?6, yaw = ?7, scale = ?8, \
                             source_revision = source_revision + 1 \
                         WHERE object_id = ?9 AND source_revision = ?10",
                        params![
                            transform.space.0,
                            transform.owner_cell.x,
                            transform.owner_cell.z,
                            transform.local_translation[0],
                            transform.local_translation[1],
                            transform.local_translation[2],
                            transform.yaw,
                            transform.scale,
                            object.0.as_slice(),
                            expected_source_revision,
                        ],
                    )?;
                    (*object, updated)
                }
                SourceObjectWrite::Delete {
                    object,
                    expected_source_revision,
                } => {
                    let updated = transaction.execute(
                        "DELETE FROM object_placements \
                         WHERE object_id = ?1 AND source_revision = ?2",
                        params![object.0.as_slice(), expected_source_revision],
                    )?;
                    (*object, updated)
                }
            };
            if updated == 0 {
                let actual = transaction
                    .query_row(
                        "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, \
                                definition_id, local_x, local_y, local_z, yaw, scale, \
                                source_revision \
                         FROM object_placements WHERE object_id = ?1",
                        params![object.0.as_slice()],
                        source_object_from_row,
                    )
                    .optional()?;
                transaction.rollback()?;
                return Ok(ObjectWriteTransactionResult::Conflict { object, actual });
            }

            match write {
                SourceObjectWrite::Create { .. } | SourceObjectWrite::UpdateTransform { .. } => {
                    transaction.execute(
                        "DELETE FROM object_cell_overlaps WHERE object_id = ?1",
                        params![object.0.as_slice()],
                    )?;
                    let updated_object = transaction.query_row(
                        "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, \
                                definition_id, local_x, local_y, local_z, yaw, scale, \
                                source_revision \
                         FROM object_placements WHERE object_id = ?1",
                        params![object.0.as_slice()],
                        source_object_from_row,
                    )?;
                    commits.push(SourceObjectWriteCommit::Updated(updated_object));
                }
                SourceObjectWrite::Delete { .. } => {
                    commits.push(SourceObjectWriteCommit::Deleted(object));
                }
            }
        }
        transaction.commit()?;
        Ok(ObjectWriteTransactionResult::Committed(commits))
    }
}

fn validate_object_transform(transform: SourceObjectTransform) -> Result<(), WorldDbError> {
    if !transform.local_translation.into_iter().all(f32::is_finite)
        || !transform.yaw.is_finite()
        || !transform.scale.is_finite()
        || transform.scale <= 0.0
    {
        return Err(WorldDbError::InvalidObjectTransform);
    }
    Ok(())
}
