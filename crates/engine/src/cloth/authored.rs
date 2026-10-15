//! Garments authored offline (`tools/cloth/author_test_cape.py`): a sheet draped over the body
//! in Blender, in a pose of its own (arms down: draped over the bind pose's raised arms a cape
//! tents out from them). The solver moves its coarse grid of particles; the mesh drawn is the
//! fine drape, each vertex skinned bilinearly to the four particles of its grid cell, so the
//! folds draped there ride on the simulation.
use super::outfit::MeshData;
use bevy::math::{Affine3A, Vec3A};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const VERSION: u32 = 1;

/// A draped sheet, bind space (glTF axes: Y up, the character facing +Z).
#[derive(Clone, Debug, Deserialize)]
pub struct Garment {
    version: u32,
    pub(crate) columns: usize,
    pub(crate) rows: usize,
    /// Each bone's skinning transform in the pose it was draped in, an affine's columns.
    #[serde(default)]
    pose: HashMap<String, [f32; 12]>,
    pub(crate) particles: Vec<GarmentParticle>,
    mesh: GarmentMesh,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GarmentParticle {
    /// Bind space: where its bones carry it from.
    pub rest: [f32; 3],
    /// As draped, in the pose it was draped in; the mesh is in this space too.
    #[serde(default)]
    pub drape: Option<[f32; 3]>,
    /// Body bones that carry it, by name, weighted.
    pub bones: Vec<(String, f32)>,
    pub pinned: bool,
    pub max_distance: f32,
    pub goal: f32,
    pub exposure: f32,
}

#[derive(Clone, Debug, Deserialize)]
struct GarmentMesh {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    /// Where each vertex lies on the particle grid: (column, row), fractional.
    grid: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Garment {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let garment: Self =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        garment
            .check()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(garment)
    }

    fn check(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!("version {} (expected {VERSION})", self.version));
        }
        if self.columns < 3 || self.rows < 3 || self.particles.len() != self.columns * self.rows {
            return Err(format!(
                "{} particles for a {}×{} grid",
                self.particles.len(),
                self.columns,
                self.rows
            ));
        }
        let mesh = &self.mesh;
        let n = mesh.positions.len();
        if mesh.normals.len() != n || mesh.uvs.len() != n || mesh.grid.len() != n {
            return Err("mesh attributes differ in length".into());
        }
        if !mesh.indices.len().is_multiple_of(3) || mesh.indices.iter().any(|&i| i as usize >= n) {
            return Err("bad mesh indices".into());
        }
        Ok(())
    }

    pub(crate) fn rest(&self, i: usize) -> Vec3A {
        Vec3A::from_array(self.particles[i].rest)
    }

    /// A bone's skinning transform in the pose draped in (the bind pose if not given).
    pub(crate) fn pose(&self, bone: &str) -> Affine3A {
        self.pose
            .get(bone)
            .map_or(Affine3A::IDENTITY, Affine3A::from_cols_array)
    }

    pub(crate) fn drape(&self, i: usize) -> Vec3A {
        Vec3A::from_array(self.particles[i].drape.unwrap_or(self.particles[i].rest))
    }

    /// The drawn mesh, its joints the grid's particles.
    pub(crate) fn mesh(&self) -> MeshData {
        let mesh = &self.mesh;
        let (columns, rows) = (self.columns, self.rows);
        let mut data = MeshData {
            positions: mesh.positions.clone(),
            normals: mesh.normals.clone(),
            uvs: mesh.uvs.clone(),
            indices: mesh.indices.clone(),
            ..MeshData::default()
        };
        for &[u, v] in &mesh.grid {
            let c0 = (u.max(0.0).floor() as usize).min(columns - 2);
            let r0 = (v.max(0.0).floor() as usize).min(rows - 2);
            let (fu, fv) = (
                (u - c0 as f32).clamp(0.0, 1.0),
                (v - r0 as f32).clamp(0.0, 1.0),
            );
            let at = |r: usize, c: usize| (r * columns + c) as u16;
            data.joints.push([
                at(r0, c0),
                at(r0, c0 + 1),
                at(r0 + 1, c0),
                at(r0 + 1, c0 + 1),
            ]);
            data.weights.push([
                (1.0 - fu) * (1.0 - fv),
                fu * (1.0 - fv),
                (1.0 - fu) * fv,
                fu * fv,
            ]);
        }
        data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn garment() -> Garment {
        let particle = |x: f32, y: f32| GarmentParticle {
            rest: [x, y, -0.2],
            drape: None,
            bones: vec![("spine".into(), 1.0)],
            pinned: y == 0.0,
            max_distance: 0.1,
            goal: 0.0,
            exposure: 1.0,
        };
        Garment {
            version: VERSION,
            columns: 3,
            rows: 3,
            pose: HashMap::new(),
            particles: (0..9)
                .map(|i| particle((i % 3) as f32, -((i / 3) as f32)))
                .collect(),
            mesh: GarmentMesh {
                positions: vec![[0.5, -1.5, -0.2], [2.0, -2.0, -0.2]],
                normals: vec![[0.0, 0.0, -1.0]; 2],
                uvs: vec![[0.0, 0.0]; 2],
                grid: vec![[0.5, 1.5], [2.0, 2.0]],
                indices: vec![0, 1, 0],
            },
        }
    }

    #[test]
    fn a_vertex_is_skinned_to_its_cell_corners() {
        let garment = garment();
        garment.check().unwrap();
        let mesh = garment.mesh();
        // In the middle of the cell from column 0 to 1, row 1 to 2.
        assert_eq!(mesh.joints[0], [3, 4, 6, 7]);
        assert_eq!(mesh.weights[0], [0.25; 4]);
        // The last corner falls in the last cell, wholly on its far corner.
        assert_eq!(mesh.joints[1], [4, 5, 7, 8]);
        assert_eq!(mesh.weights[1], [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_grid_that_does_not_match_its_particles_is_refused() {
        let mut garment = garment();
        garment.rows = 4;
        assert!(garment.check().is_err());
    }
}
