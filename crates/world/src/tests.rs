use super::*;

#[test]
fn negative_coordinates_use_floor_cells() {
    assert_eq!(
        CellCoord::containing(-0.01, -32.0, 32.0),
        CellCoord { x: -1, z: -1 }
    );
    assert_eq!(
        CellCoord::containing(-32.01, 31.99, 32.0),
        CellCoord { x: -2, z: 0 }
    );
}

#[test]
fn world_position_is_normalized_and_relative() {
    let position = WorldPosition::from_world(WorldSpaceId(7), [-32.5, 4.0, 64.25], 32.0);
    assert_eq!(position.cell, CellCoord { x: -2, z: 2 });
    assert_eq!(position.local, [31.5, 4.0, 0.25]);
    assert_eq!(
        position.relative_to(CellCoord { x: -1, z: 1 }, 32.0),
        [-0.5, 4.0, 32.25]
    );
    assert_eq!(position.world(32.0), [-32.5, 4.0, 64.25]);
}

#[test]
fn world_position_translation_stays_precise_far_from_render_origin() {
    let position = WorldPosition {
        space: WorldSpaceId(7),
        cell: CellCoord {
            x: 1_000_000,
            z: -1_000_000,
        },
        local: [31.5, 4.0, 0.25],
    };
    let moved = position.translated([1.0, 2.0, -1.0], 32.0);

    assert_eq!(
        moved.cell,
        CellCoord {
            x: 1_000_001,
            z: -1_000_001,
        }
    );
    assert_eq!(moved.local, [0.5, 6.0, 31.25]);
    assert_eq!(moved.relative_to(position.cell, 32.0), [32.5, 6.0, -0.75]);
}

#[test]
fn far_object_blocks_floor_negative_cells() {
    let block = |x, z| far_object_block(CellCoord { x, z });
    assert_eq!(block(0, 15), CellCoord { x: 0, z: 0 });
    assert_eq!(block(-1, 16), CellCoord { x: -1, z: 1 });
    assert_eq!(block(-16, -17), CellCoord { x: -1, z: -2 });
}

#[test]
fn far_objects_page_round_trips_and_rejects_dangling_forms() {
    let mut page = FarObjectsPage {
        forms: vec![FarObjectForm {
            uri: "packs/tree.impostor.json".into(),
            switch: 0.1,
            height: 20.0,
            meshes: vec![FarObjectMesh {
                uri: "packs/tree_lod0.gltf".into(),
                minimum_screen_height: 200.0,
            }],
        }],
        instances: vec![FarObjectInstance {
            form: 0,
            translation: [1.0, 2.0, 3.0],
            yaw: 0.5,
            scale: 1.5,
        }],
    };
    page.validate().unwrap();
    assert_eq!(
        FarObjectsPage::decode(&page.encode().unwrap()).unwrap(),
        page
    );
    page.instances[0].form = 1;
    assert!(page.validate().is_err());
    page.instances[0].form = 0;
    page.forms[0].meshes[0].uri = "packs/tree.impostor.json".into();
    assert!(page.validate().is_err());
    page.forms[0].meshes.clear();
    assert!(page.validate().is_err());
    page.forms[0].uri = "packs/tree.gltf".into();
    assert!(page.validate().is_err());
}

#[test]
fn heightfield_payload_round_trips_and_samples_relief() {
    let heightfield = TerrainHeightfield::from_heights(
        3,
        &[0.0, 1.0, 2.0, 1.0, 2.0, 3.0, 2.0, 3.0, 4.0],
        0.0,
        4.0,
        4.0,
    )
    .unwrap();
    let payload = PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
        heightfield: heightfield.clone(),
        surfaces: vec![TerrainSurfaceId([4; 16])],
        wear: None,
    });

    let mut bytes = encode_page_payload(&payload).unwrap();
    assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
    assert_eq!(payload.domain(), PageDomain::Terrain);
    bytes.push(0);
    assert!(matches!(
        decode_page_payload(&bytes),
        Err(DecodeError::TrailingBytes { .. })
    ));

    let center = heightfield.sample([2.0, 2.0], 4.0);
    assert!((center.height - 2.0).abs() < 0.001);
    assert!(center.normal[0] < -0.4 && center.normal[2] < -0.4);
    assert!(center.normal[1] > 0.5);
}

#[test]
fn source_height_precision_keeps_adjacent_edges_identical() {
    let left =
        TerrainHeightfield::from_heights(2, &[0.0, 1.25, 0.5, 1.75], -8.0, 8.0, 32.0).unwrap();
    let right =
        TerrainHeightfield::from_heights(2, &[1.25, 2.0, 1.75, 2.5], -8.0, 8.0, 32.0).unwrap();

    assert_eq!(left.heights[1], right.heights[0]);
    assert_eq!(left.heights[3], right.heights[2]);
    assert_eq!(left.height_at(1, 0), right.height_at(0, 0));
    assert_eq!(left.height_at(1, 1), right.height_at(0, 1));
}

#[test]
fn mountain_range_preserves_shallow_road_relief() {
    let heights = [1500.0, 1499.995, 1499.99, 1500.0];
    let field = TerrainHeightfield::from_heights(2, &heights, -500.0, 2500.0, 8.0).unwrap();

    // Millimetre rounding keeps 5 mm steps at 1.5 km, where a per-page 16-bit range
    // over this world's 3 km would round to 4.6 cm.
    for (stored, source) in field.heights.iter().zip(heights) {
        assert!((stored - source).abs() <= TERRAIN_HEIGHT_STEP / 2.0);
    }

    assert!(field.heights[0] > field.heights[1] && field.heights[1] > field.heights[2]);
    assert_eq!(
        field.height_bounds(),
        [quantize_terrain_height(1499.99), 1500.0]
    );
    let mut invalid = field.clone();
    invalid.heights[1] = f32::NAN;
    assert!(invalid.validate().is_err());
}

#[test]
fn terrain_and_vegetation_follow_both_triangles_of_a_saddle() {
    let terrain =
        TerrainHeightfield::from_heights(2, &[0.0, 10.0, 20.0, 0.0], 0.0, 20.0, 1.0).unwrap();
    let surface = vegetation::VegetationSurfaceField {
        resolution: 2,
        heights: terrain.heights.clone(),
        normals_oct: terrain.normals_oct.clone(),
        validity: vec![255; 4],
    };

    // The rendered 00--11 diagonal is zero; bilinear sampling would return 7.5.
    for (xz, expected) in [
        ([0.5, 0.5], 0.0),
        ([0.75, 0.25], 5.0),
        ([0.25, 0.75], 10.0),
        ([1.0, 1.0], 0.0),
        ([1.0, 0.5], 5.0),
    ] {
        let ground = terrain.sample(xz, 1.0);
        let grass = surface.sample([0.0; 2], 1.0, xz);
        assert_eq!(ground.height, expected);
        assert_eq!(ground.height, grass.height);

        for axis in 0..3 {
            assert!((ground.normal[axis] - grass.normal[axis]).abs() < 1e-6);
        }
    }
}

#[test]
fn gameplay_page_keeps_definition_identity() {
    let definition = ObjectDefinitionId([3; 16]);
    let payload = PagePayload::GameplayObjects(GameplayObjectsPage {
        instances: vec![GameplayObjectInstance {
            id: StableObjectId([7; 16]),
            definition,
            translation: [2.0, 0.0, 4.0],
            yaw: 0.5,
            scale: 1.0,
        }],
    });
    let bytes = encode_page_payload(&payload).unwrap();
    assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
    assert_eq!(payload.domain(), PageDomain::GameplayObjects);
}

#[test]
fn vegetation_page_round_trips_without_duplicating_terrain_relief() {
    let data = vegetation::fixtures::reference_page([0.0, 0.0]).data();
    let payload = PagePayload::Vegetation(data.clone());

    let bytes = encode_page_payload(&payload).unwrap();
    assert_eq!(decode_page_payload(&bytes).unwrap(), payload);
    assert_eq!(payload.domain(), PageDomain::Vegetation);
    assert_eq!(data.fields.len(), 2);
}
