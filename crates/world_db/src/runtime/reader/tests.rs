use super::*;
use crate::{atmosphere, schema};

#[test]
fn runtime_cell_windows_are_bounded_and_seek_both_spatial_axes() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(schema::RUNTIME_SCHEMA).unwrap();
    connection
        .execute(
            "INSERT INTO world_spaces VALUES (1,'test',8,-1,1,?1,1)",
            [atmosphere::encode(&Default::default()).unwrap()],
        )
        .unwrap();
    for x in -2..=2 {
        for z in [-10000, -2, -1, 0, 1, 2, 10000] {
            connection
                .execute("INSERT INTO cells VALUES (1,?1,?2,-1,1,0,1)", params![x, z])
                .unwrap();
        }
    }
    let reader = RuntimeReader {
        connection,
        manifest: RuntimeManifest {
            schema_version: RUNTIME_SCHEMA_VERSION,
            generation_id: "test".into(),
            content_hash: [0; 32],
            default_world_space: WorldSpaceId(1),
            world_spaces: vec![],
            vegetation_catalog: None,
        },
    };
    let rows = reader
        .read_cell_descriptors(
            WorldSpaceId(1),
            CellCoord { x: -1, z: -1 },
            CellCoord { x: 1, z: 1 },
        )
        .unwrap();
    assert_eq!(rows.len(), 9);
    assert!(rows.windows(2).all(|pair| pair[0].cell < pair[1].cell));
    assert!(
        rows.iter()
            .all(|d| d.cell.x.abs() <= 1 && d.cell.z.abs() <= 1)
    );
    assert!(
        reader
            .read_cell_descriptors(
                WorldSpaceId(1),
                CellCoord { x: -32, z: -32 },
                CellCoord { x: 31, z: 31 }
            )
            .is_ok()
    );
    assert!(matches!(
        reader.read_cell_descriptors(
            WorldSpaceId(1),
            CellCoord { x: -32, z: -32 },
            CellCoord { x: 32, z: 32 }
        ),
        Err(WorldDbError::InvalidQueryLimit)
    ));
    assert!(
        reader
            .read_cell_descriptors(WorldSpaceId(1), CellCoord { x: 1, z: 0 }, CellCoord::ZERO)
            .is_err()
    );
    assert!(
        reader
            .read_cell_descriptors(
                WorldSpaceId(1),
                CellCoord {
                    x: i32::MIN,
                    z: i32::MIN
                },
                CellCoord {
                    x: i32::MAX,
                    z: i32::MAX
                }
            )
            .is_err()
    );
    let plan: String = reader
        .connection
        .prepare(&format!("EXPLAIN QUERY PLAN {CELL_DESCRIPTOR_SQL}"))
        .unwrap()
        .query_row(params![1, -1, -1, 1], |row| row.get(3))
        .unwrap();
    assert!(
        plan.contains("PRIMARY KEY") && plan.contains("cell_x=?") && plan.contains("cell_z>?"),
        "{plan}"
    );
}
