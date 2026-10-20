//! The compiler's view of coverage: the source's own index (`environment::CoverageIndex`),
//! loaded within the plan's budgets, and each cell's input fingerprint.
use environment::{CoverageIndex, CoverageLimits, CoverageSnapshot};
use world::CellCoord;

use crate::{CompileError, CompilePlan};

pub(crate) type Coverage<'a> = CoverageIndex<'a>;

impl CompilePlan {
    pub(crate) fn coverage_limits(&self) -> CoverageLimits {
        CoverageLimits {
            cells: self.profile.max_cells_per_batch,
            mask_samples: self.profile.max_mask_samples,
        }
    }

    pub(crate) fn coverage<'a>(
        &self,
        source: &'a CoverageSnapshot,
    ) -> Result<Coverage<'a>, CompileError> {
        Ok(CoverageIndex::new(
            &self.definition,
            source,
            self.coverage_limits(),
        )?)
    }
}

/// What a cell's compiled output depends on besides roads: the plan, and the tiles and
/// revisions of the cell and its halo.
pub(crate) fn fingerprint(
    coverage: &Coverage,
    plan: &CompilePlan,
    cell: CellCoord,
) -> Result<[u8; 32], CompileError> {
    let mut hash = blake3::Hasher::new();
    hash.update(&plan.fingerprint);
    hash.update(&cell.x.to_le_bytes());
    hash.update(&cell.z.to_le_bytes());

    for (_, _, neighbour) in halo(cell)? {
        let source = coverage
            .cell(neighbour)
            .ok_or(environment::CoverageError::UnloadedCell(neighbour))?;

        // BTreeMap gives a canonical tile order, including explicit absent-tile information.
        let bytes = world::codec::encode(&(neighbour, source.source.revision, &source.tiles))?;
        hash.update(&bytes);
    }

    Ok(*hash.finalize().as_bytes())
}

pub(crate) fn halo(cell: CellCoord) -> Result<Vec<(i32, i32, CellCoord)>, CompileError> {
    Ok(environment::halo(cell)?)
}
