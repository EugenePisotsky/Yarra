//! One cell drawn by the clipmap material outside streaming: the editor's preset preview. A
//! one-level clipmap holds the cell as its only node, so the preview shades its ground, roads
//! and wear exactly as the game does.
use super::{
    ClipmapGround, ClipmapLayout, ClipmapTerrainMaterial, ClipmapTextures, ClipmapUploads,
    LodRanges, NodeBlock, NodeWindow, Residency, WEAR_SLOTS, patch_mesh, patch_tag,
    patch_transform, wear_slot,
};

use bevy::{
    camera::{primitives::Aabb, visibility::NoAutoAabb},
    mesh::MeshTag,
    prelude::*,
    render::storage::ShaderBuffer,
};

use world::{CellCoord, TerrainNode, TerrainNodeKey, TerrainWear};

/// Texels per side: the node's block and its guard.
const SIDE: u32 = 64;

/// The cell is the only level, so it never morphs.
const RANGES: LodRanges = LodRanges { leaf: 1.0e9 };

pub struct ClipmapPatch {
    textures: ClipmapTextures,
    key: TerrainNodeKey,
    bounds: [f32; 2],
    windows: [NodeWindow; 1],
    residency: Residency,
    wear: [Option<CellCoord>; WEAR_SLOTS],
}

/// The assets a patch makes its textures and material in.
pub struct PatchAssets<'a> {
    pub images: &'a mut Assets<Image>,
    pub buffers: &'a mut Assets<ShaderBuffer>,
    pub materials: &'a mut Assets<ClipmapTerrainMaterial>,
}

impl ClipmapPatch {
    /// A level-0 `node` of a `cell_size` cell, its control tile (`world::leaf_control`) and
    /// its road wear; the node's samples set the clipmap's.
    pub fn new(
        node: &TerrainNode,
        cell_size: f32,
        control: Vec<u32>,
        wear: Option<&TerrainWear>,
        ground: ClipmapGround,
        uploads: &mut ClipmapUploads,
        assets: PatchAssets,
    ) -> Result<Self, String> {
        let field = node
            .heightfield
            .as_ref()
            .ok_or("the previewed cell has no ground")?;
        let intervals = u32::from(field.resolution) - 1;

        if node.key.level != 0 || !intervals.is_power_of_two() || 2 * intervals > SIDE {
            return Err(format!(
                "a preview cell is one level-0 node of up to {} samples a side",
                SIDE / 2 + 1
            ));
        }

        let layout = ClipmapLayout::new(SIDE, 1, f64::from(cell_size), intervals);
        let block = NodeBlock::relief_of(node, intervals)?.with_control(control);
        let textures = ClipmapTextures::new(
            layout,
            RANGES,
            ground,
            assets.images,
            assets.buffers,
            assets.materials,
        );
        uploads.target(&textures);
        uploads.push(&textures, block);
        let mut wear_cells = [None; WEAR_SLOTS];
        let cell = CellCoord {
            x: node.key.x,
            z: node.key.z,
        };

        if let Some(wear) = wear.filter(|w| w.resolution == world::WEAR_RESOLUTION) {
            uploads.push_wear(&textures, wear_slot(cell), wear);
            wear_cells[wear_slot(cell)] = Some(cell);
        }

        let mut residency = Residency::new(1);
        residency.set(&layout, node.key, true);
        Ok(Self {
            textures,
            key: node.key,
            bounds: node.height_bounds,
            windows: [NodeWindow {
                level: 0,
                min: [node.key.x, node.key.z],
                size: 1,
            }],
            residency,
            wear: wear_cells,
        })
    }

    /// The patch entity's mesh, material and placement, with the render origin at the
    /// world's.
    pub fn bundle(&self, meshes: &mut Assets<Mesh>) -> impl Bundle + use<> {
        let layout = &self.textures.layout;
        let n = layout.intervals as f32;
        (
            Mesh3d(meshes.add(patch_mesh(layout))),
            MeshMaterial3d(self.textures.material.clone()),
            MeshTag::new(patch_tag(self.key)),
            patch_transform(layout, self.key, [0.0; 2]),
            Aabb::from_min_max(
                Vec3::new(0.0, self.bounds[0], 0.0),
                Vec3::new(n, self.bounds[1], n),
            ),
            NoAutoAabb,
        )
    }

    /// The shaders' frame for a view at `eye` (render space, whose origin is the world's).
    pub fn follow(&self, eye: Vec3, uploads: &mut ClipmapUploads) {
        uploads.set_frame(
            &self.textures,
            &super::ClipmapFrame {
                eye,
                origin_samples: [0; 2],
                windows: &self.windows,
                residency: &self.residency,
                wear: &self.wear,
            },
        );
    }

    /// Stops its uploads; its textures and material go with their last handles.
    pub fn remove(
        self,
        uploads: &mut ClipmapUploads,
        materials: &mut Assets<ClipmapTerrainMaterial>,
    ) {
        uploads.forget(&self.textures);
        materials.remove(self.textures.material.id());
    }
}
