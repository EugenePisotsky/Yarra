//! Pure grass contracts shared by cooking, rendering, editor tooling and tests: grass types and
//! what grows on each ground surface, the coverage fields cooked per world cell, and the
//! deterministic blade placement the GPU reproduces.
//!
//! This crate deliberately has no Bevy, SQLite, or GPU dependency. It defines what grass means;
//! integration crates decide how records are stored, streamed, and rendered.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod fixtures;
mod placement;

pub use placement::*;

/// A grass type's stable identity, derived from its key so packs may reorder types freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GrassTypeId(pub [u8; 16]);

impl GrassTypeId {
    /// Two FNV-1a passes with different offsets: stable across builds and platforms.
    pub fn from_key(key: &str) -> Self {
        let pass = |offset: u64| {
            key.bytes().fold(offset, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            })
        };
        let mut id = [0; 16];
        id[..8].copy_from_slice(&pass(0xcbf2_9ce4_8422_2325).to_le_bytes());
        id[8..].copy_from_slice(&pass(0x6c62_272e_07bb_0142).to_le_bytes());
        Self(id)
    }

    /// The placement seed: every type grows on its own world-aligned lattice.
    pub fn seed(self) -> u32 {
        u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
}

/// How blades group into clumps: Voronoi cells of `size`, each with its own height, colour
/// and facing, as in Ghost of Tsushima's grass.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GrassClumps {
    /// Metres between clump centres.
    pub size: f32,
    /// Share of the way each root moves toward its clump centre: 0 spreads blades evenly,
    /// 1 gathers them into tight tufts with bare ground between.
    pub pull: f32,
    /// How much clumps differ in height: 0 all alike, 1 from half to full height.
    pub height: f32,
    /// Share of blades that face their clump's common direction instead of their own.
    pub facing: f32,
    /// How strongly blades splay outward from their clump centre.
    pub splay: f32,
    /// How partial coverage thins the type: 0 drops single blades evenly, 1 drops whole clumps,
    /// so sparse grass grows as separate tussocks.
    pub patches: f32,
}

/// Linear-light colours of a blade along its length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GrassColors {
    pub root: [f32; 3],
    pub tip: [f32; 3],
    /// The tip colour of dry clumps.
    pub dry_tip: [f32; 3],
    /// Share of clumps with dry tips.
    pub dry: f32,
    /// Clump-to-clump brightness variation, as a share.
    pub variation: f32,
}

impl GrassColors {
    /// The mean colour a field of these blades shows from a distance, where their upper parts
    /// hide the roots.
    pub fn field(&self) -> [f32; 3] {
        std::array::from_fn(|i| {
            let tip = self.tip[i] + (self.dry_tip[i] - self.tip[i]) * self.dry;
            self.root[i] + (tip - self.root[i]) * 0.7
        })
    }
}

/// What the grass on a ground surface makes of it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GrassCover {
    /// How fully grass covers the surface, 0..1: its types' shares, a type at
    /// `VegetationCatalog::FULL_COVER_DENSITY` or more counting fully, and tufted types (high
    /// `pull`) leaving bare ground between their tufts.
    pub amount: f32,
    /// Linear colour of the field from a distance, its types mixed by their cover.
    pub color: [f32; 3],
}

impl GrassCover {
    /// Colour, then amount.
    pub fn rgba(self) -> [f32; 4] {
        [self.color[0], self.color[1], self.color[2], self.amount]
    }
}

/// One kind of grass: blade shape, clumping, colour, lighting and wind response. Every blade
/// picks its values within the ranges, its clump shared by its neighbours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrassType {
    pub key: String,
    /// Roots per square metre where the type grows at full amount; twin types grow two blades
    /// from each.
    pub density: f32,
    /// Blade height in metres.
    pub height: [f32; 2],
    /// Blade width at the root in metres; blades taper to a point.
    pub width: [f32; 2],
    /// How far the tip leans from upright: 0 upright, 1 lying along the ground.
    pub tilt: [f32; 2],
    /// How much the blade's middle arches above the line from root to tip.
    pub bend: [f32; 2],
    pub clumps: GrassClumps,
    pub colors: GrassColors,
    /// Strength of the sun's sheen on the blades.
    pub gloss: f32,
    pub roughness: f32,
    /// Share of the sun passing through a blade seen against the light.
    pub translucency: f32,
    /// Resistance to wind: 0 sways freely, 1 barely moves.
    pub stiffness: f32,
    /// Each root grows two blades in a V that share one blade's vertex budget, as in Ghost of
    /// Tsushima: twice the blades for the same placement and vertex work, with fewer bends each.
    pub twin: bool,
}

impl Lattice for GrassType {
    fn seed(&self) -> u32 {
        self.id().seed()
    }

    fn spacing(&self) -> f32 {
        GrassType::spacing(self)
    }

    fn clumps(&self) -> GrassClumps {
        self.clumps
    }
}

impl GrassType {
    pub fn id(&self) -> GrassTypeId {
        GrassTypeId::from_key(&self.key)
    }

    /// Metres between lattice candidates.
    pub fn spacing(&self) -> f32 {
        self.density.sqrt().recip()
    }

    /// Blades per square metre where the type grows at full amount.
    pub fn blades_per_square_metre(&self) -> f32 {
        self.density * if self.twin { 2.0 } else { 1.0 }
    }

    fn is_valid(&self) -> bool {
        let range = |r: [f32; 2], minimum: f32, maximum: f32| {
            finite_range(r[0], minimum, maximum) && finite_range(r[1], r[0], maximum)
        };
        let color = |c: [f32; 3]| c.into_iter().all(|v| finite_range(v, 0.0, 1.0));
        let c = self.clumps;
        !self.key.is_empty()
            && finite_range(self.density, 0.25, 400.0)
            && range(self.height, 0.02, 3.0)
            && range(self.width, 0.0005, 0.1)
            && range(self.tilt, 0.0, 1.0)
            && range(self.bend, -0.5, 1.0)
            && finite_range(c.size, 0.1, 8.0)
            && [c.pull, c.height, c.facing, c.splay, c.patches]
                .into_iter()
                .all(|v| finite_range(v, 0.0, 1.0))
            && color(self.colors.root)
            && color(self.colors.tip)
            && color(self.colors.dry_tip)
            && finite_range(self.colors.dry, 0.0, 1.0)
            && finite_range(self.colors.variation, 0.0, 1.0)
            && finite_range(self.gloss, 0.0, 2.0)
            && finite_range(self.roughness, 0.05, 1.0)
            && finite_range(self.translucency, 0.0, 1.0)
            && finite_range(self.stiffness, 0.0, 1.0)
    }
}

/// Small objects strewn over the ground: stones, pebbles, twigs, ferns and herbs. Like grass they
/// grow where their ground surfaces say (`SurfaceGrass`, sharing the grass types' ID space), on a
/// world-aligned lattice gathered into clumps; each object is a random piece of the type's assets,
/// glTF files whose meshes are its pieces (`tools/import_detail_props.py` for props; plants from
/// vegetation bundles, swaying and shaded like tree foliage by their material tags), standing on
/// the origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetailType {
    pub key: String,
    /// glTF files under `assets/`; every mesh in them is a piece.
    pub assets: Vec<String>,
    /// Objects per square metre where the type grows at full amount.
    pub density: f32,
    /// Scale applied to the pieces as authored, (minimum, maximum).
    pub scale: [f32; 2],
    /// Voronoi clumps as grass's: `size` metres apart, `pull` gathering objects to their centres,
    /// `patches` making partial coverage keep or drop whole clumps.
    pub clumps: DetailClumps,
    /// Share of each piece's height sunk into the ground.
    pub sink: f32,
    /// How far pieces follow the ground's slope: 0 upright, 1 along its normal.
    pub align: f32,
    /// Random tilt from that, radians.
    pub tilt: f32,
    /// Metres from the camera at which they have faded out.
    pub distance: f32,
    /// Whether they cast shadows; pebbles and twigs are too small to need them.
    pub shadows: bool,
    /// Whether they lie on worn ground too: road wear and grass exclusions, which clear plants,
    /// leave stones and twigs where the ground carries them.
    #[serde(default)]
    pub on_paths: bool,
    /// Where on worn ground the type grows, by how worn it is (roads' and junctions' wear,
    /// `world::TerrainWear`): rising from the first wear to full at the second and gone by the
    /// third (kept above the second if the third equals it). Stones kicked aside where wear falls off, weeds in moderately trampled ground. A
    /// type with a band grows only there, its surface shares counting the natural ground under
    /// the wear; without, it grows on the ground that shows.
    #[serde(default)]
    pub wear: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DetailClumps {
    pub size: f32,
    pub pull: f32,
    pub patches: f32,
}

/// Ground details stop drawing within this distance at most.
pub const MAX_DETAIL_DISTANCE: f32 = 160.0;

impl DetailType {
    pub fn id(&self) -> GrassTypeId {
        GrassTypeId::from_key(&self.key)
    }

    fn is_valid(&self) -> bool {
        let c = self.clumps;
        !self.key.is_empty()
            && !self.assets.is_empty()
            && self.assets.iter().all(|a| !a.is_empty())
            && finite_range(self.density, 0.001, 50.0)
            && finite_range(self.scale[0], 0.05, 20.0)
            && finite_range(self.scale[1], self.scale[0], 20.0)
            && finite_range(c.size, 0.1, 16.0)
            && finite_range(c.pull, 0.0, 1.0)
            && finite_range(c.patches, 0.0, 1.0)
            && finite_range(self.sink, 0.0, 1.0)
            && finite_range(self.align, 0.0, 1.0)
            && finite_range(self.tilt, 0.0, 1.6)
            && finite_range(self.distance, 1.0, MAX_DETAIL_DISTANCE)
            && self.wear.is_none_or(|[from, peak, to]| {
                finite_range(from, 0.0, 1.0)
                    && finite_range(peak, from + 0.01, 1.0)
                    && finite_range(to, peak, 1.0)
            })
    }
}

/// The share of a type with the wear band `band` (`DetailType::wear`) that grows at `wear` (0..1).
pub fn wear_share(band: [f32; 3], wear: f64) -> f64 {
    let [from, peak, to] = band.map(f64::from);
    let smooth = |t: f64| {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };

    if wear <= peak {
        smooth((wear - from) / (peak - from))
    } else if to > peak {
        1.0 - smooth((wear - peak) / (to - peak))
    } else {
        1.0
    }
}

impl Lattice for DetailType {
    fn seed(&self) -> u32 {
        self.id().seed()
    }

    fn spacing(&self) -> f32 {
        self.density.sqrt().recip()
    }

    fn clumps(&self) -> GrassClumps {
        GrassClumps {
            size: self.clumps.size,
            pull: self.clumps.pull,
            height: 0.0,
            facing: 0.0,
            splay: 0.0,
            patches: self.clumps.patches,
        }
    }
}

/// How much of a grass or detail type grows on a surface: a share of the type's density.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GrassAmount {
    pub grass: GrassTypeId,
    pub amount: f32,
}

/// What grows on one ground surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceGrass {
    /// A `world::TerrainSurfaceId`, by value: this crate sits below `world`.
    pub surface: [u8; 16],
    pub grass: Vec<GrassAmount>,
}

pub const MAX_GRASS_TYPES: usize = 256;

/// The grass of a world: its types and what grows on each of its ground surfaces.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VegetationCatalog {
    pub grass: Vec<GrassType>,
    pub details: Vec<DetailType>,
    pub surfaces: Vec<SurfaceGrass>,
}

impl VegetationCatalog {
    pub fn validate(&self) -> Result<(), CatalogValidationError> {
        if self.grass.len() + self.details.len() > MAX_GRASS_TYPES {
            return Err(CatalogValidationError::TooManyTypes);
        }

        let mut ids = HashSet::new();

        for grass in &self.grass {
            if !ids.insert(grass.id()) {
                return Err(CatalogValidationError::DuplicateType(grass.key.clone()));
            }
            if !grass.is_valid() {
                return Err(CatalogValidationError::InvalidType(grass.key.clone()));
            }
        }

        for detail in &self.details {
            if !ids.insert(detail.id()) {
                return Err(CatalogValidationError::DuplicateType(detail.key.clone()));
            }
            if !detail.is_valid() {
                return Err(CatalogValidationError::InvalidType(detail.key.clone()));
            }
        }

        let mut surfaces = HashSet::new();

        for surface in &self.surfaces {
            if !surfaces.insert(surface.surface) {
                return Err(CatalogValidationError::DuplicateSurface(surface.surface));
            }

            let mut types = HashSet::new();

            for amount in &surface.grass {
                if !ids.contains(&amount.grass)
                    || !types.insert(amount.grass)
                    || !finite_range(amount.amount, 0.0, 4.0)
                {
                    return Err(CatalogValidationError::InvalidSurface(surface.surface));
                }
            }
        }

        Ok(())
    }

    pub fn grass(&self, id: GrassTypeId) -> Option<&GrassType> {
        self.grass.iter().find(|grass| grass.id() == id)
    }

    pub fn detail(&self, id: GrassTypeId) -> Option<&DetailType> {
        self.details.iter().find(|detail| detail.id() == id)
    }

    /// Every grass and detail type's ID, grass first, each in catalog order.
    pub fn type_ids(&self) -> impl Iterator<Item = GrassTypeId> + '_ {
        self.grass
            .iter()
            .map(GrassType::id)
            .chain(self.details.iter().map(DetailType::id))
    }

    pub fn index(&self, id: GrassTypeId) -> Option<usize> {
        self.grass.iter().position(|grass| grass.id() == id)
    }

    /// What grows on `surface`; nothing for a surface the catalog does not list.
    pub fn on_surface(&self, surface: [u8; 16]) -> &[GrassAmount] {
        self.surfaces
            .iter()
            .find(|s| s.surface == surface)
            .map_or(&[], |s| &s.grass)
    }

    /// Blades per square metre at which one type covers its ground fully.
    pub const FULL_COVER_DENSITY: f32 = 30.0;

    /// How fully one type covers its ground where it grows at full amount: at
    /// `FULL_COVER_DENSITY` or more fully, tufted types (high `pull`) leaving bare ground between
    /// their tufts.
    pub fn cover_share(grass: &GrassType) -> f32 {
        (grass.blades_per_square_metre() / Self::FULL_COVER_DENSITY).min(1.0)
            * (1.0 - 0.5 * grass.clumps.pull)
    }

    /// How grass covers `surface`: ground under it is shaded where blades are drawn and takes
    /// the field's colour beyond them.
    pub fn cover(&self, surface: [u8; 16]) -> GrassCover {
        let mut total = 0.0;
        let mut color = [0.0; 3];

        for a in self.on_surface(surface) {
            let Some(grass) = self.grass(a.grass) else {
                continue;
            };
            let share = a.amount * Self::cover_share(grass);
            let field = grass.colors.field();
            total += share;

            for (c, f) in color.iter_mut().zip(field) {
                *c += f * share;
            }
        }

        if total <= 0.0 {
            return GrassCover::default();
        }

        GrassCover {
            amount: total.min(1.0),
            color: color.map(|c| c / total),
        }
    }

    /// Takes the look of `other`'s types with the same keys, keeping this catalog's set of types
    /// and surfaces: what a live-edited pack may change without recooking coverage. True when a
    /// type changed.
    pub fn adopt_looks(&mut self, other: &VegetationCatalog) -> bool {
        let mut changed = false;

        for grass in &mut self.grass {
            if let Some(source) = other.grass.iter().find(|g| g.key == grass.key)
                && source != grass
            {
                *grass = source.clone();
                changed = true;
            }
        }

        for detail in &mut self.details {
            if let Some(source) = other.details.iter().find(|d| d.key == detail.key)
                && source != detail
            {
                *detail = source.clone();
                changed = true;
            }
        }

        changed
    }
}

/// A grass pack as authored in RON (`assets/packs/grass/*.ron`): its types, and what grows on
/// each ground surface of its terrain pack, by surface key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrassPack {
    /// The terrain pack whose surface keys `surfaces` names.
    pub terrain: String,
    pub types: Vec<GrassType>,
    #[serde(default)]
    pub details: Vec<DetailType>,
    /// Surface key → (grass or detail key, share of that type's density).
    pub surfaces: BTreeMap<String, Vec<(String, f32)>>,
}

impl GrassPack {
    pub fn parse(text: &str) -> Result<Self, String> {
        ron::from_str(text).map_err(|e| e.to_string())
    }

    /// The catalog for a world whose ground surface `key` has the ID `surface(key)`.
    pub fn catalog(
        &self,
        surface: impl Fn(&str) -> Option<[u8; 16]>,
    ) -> Result<VegetationCatalog, String> {
        let surfaces = self
            .surfaces
            .iter()
            .map(|(key, grass)| {
                let surface = surface(key)
                    .ok_or_else(|| format!("terrain pack {} has no surface {key}", self.terrain))?;
                let grass = grass
                    .iter()
                    .map(|(grass, amount)| {
                        (self.types.iter().any(|t| &t.key == grass)
                            || self.details.iter().any(|t| &t.key == grass))
                        .then(|| GrassAmount {
                            grass: GrassTypeId::from_key(grass),
                            amount: *amount,
                        })
                        .ok_or_else(|| format!("surface {key} names unknown grass {grass}"))
                    })
                    .collect::<Result<_, _>>()?;
                Ok(SurfaceGrass { surface, grass })
            })
            .collect::<Result<_, String>>()?;
        let catalog = VegetationCatalog {
            grass: self.types.clone(),
            details: self.details.clone(),
            surfaces,
        };
        catalog.validate().map_err(|e| e.to_string())?;
        Ok(catalog)
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CatalogValidationError {
    #[error("more than {MAX_GRASS_TYPES} grass types")]
    TooManyTypes,
    #[error("duplicate grass type {0}")]
    DuplicateType(String),
    #[error("invalid grass type {0}")]
    InvalidType(String),
    #[error("surface {0:?} is listed twice")]
    DuplicateSurface([u8; 16]),
    #[error("surface {0:?} names unknown, repeated or out-of-range grass")]
    InvalidSurface([u8; 16]),
}

/// One grass type's coverage over a page: how much of its density grows at each sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrassField {
    pub grass: GrassTypeId,
    pub resolution: u16,
    /// Row-major R8 samples at texel centres.
    pub coverage: Vec<u8>,
}

impl GrassField {
    pub fn sample_coverage(&self, page: &VegetationFieldPage, world_xz: [f32; 2]) -> f32 {
        if !page.owns(world_xz) || self.resolution == 0 {
            return 0.0;
        }

        let resolution = usize::from(self.resolution);
        let local_x = ((world_xz[0] - page.origin_xz[0]) / page.size).clamp(0.0, 0.999_999);
        let local_z = ((world_xz[1] - page.origin_xz[1]) / page.size).clamp(0.0, 0.999_999);
        let x = (local_x * resolution as f32) as usize;
        let z = (local_z * resolution as f32) as usize;
        f32::from(self.coverage[z * resolution + x]) / 255.0
    }
}

/// Persistent, terrain-independent grass data for one streamed world page.
///
/// The page key and world-space record provide its cell and extent. Relief is deliberately absent:
/// a resident vegetation page joins this data with the matching streamed terrain heightfield. This
/// avoids storing the same surface twice and guarantees placement, grounding, and rendering sample
/// one authoritative terrain page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationFieldPageData {
    pub fields: Vec<GrassField>,
}

impl VegetationFieldPageData {
    pub fn validate(&self, catalog: &VegetationCatalog) -> Result<(), PageValidationError> {
        let mut types = HashSet::new();

        for field in &self.fields {
            if catalog.grass(field.grass).is_none() && catalog.detail(field.grass).is_none() {
                return Err(PageValidationError::MissingType(field.grass));
            }

            if !types.insert(field.grass) {
                return Err(PageValidationError::DuplicateType(field.grass));
            }

            let resolution = usize::from(field.resolution);

            if resolution == 0 || resolution > 256 || field.coverage.len() != resolution.pow(2) {
                return Err(PageValidationError::InvalidField(field.grass));
            }
        }

        Ok(())
    }

    /// How grass covers the page's ground where it actually grows (roads, exclusions and painted
    /// coverage included), as `GrassCover` per texel: row-major RGBA8 at the grass fields' texel
    /// centres, the field colour's square root in RGB (dark linear colours keep their precision)
    /// and the cover in A. `None` without grass, or for fields of differing resolution.
    pub fn cover_map(&self, catalog: &VegetationCatalog) -> Option<(u16, Vec<u8>)> {
        let fields: Vec<_> = self
            .fields
            .iter()
            .filter_map(|f| {
                let grass = catalog.grass(f.grass)?;
                Some((
                    f,
                    VegetationCatalog::cover_share(grass),
                    grass.colors.field(),
                ))
            })
            .collect();
        let resolution = fields.first()?.0.resolution;

        if fields.iter().any(|(f, _, _)| f.resolution != resolution) {
            return None;
        }

        let texels = usize::from(resolution).pow(2);
        let mut rgba = Vec::with_capacity(texels * 4);

        for i in 0..texels {
            let mut total = 0.0;
            let mut color = [0.0; 3];

            for (field, share, field_color) in &fields {
                let share = f32::from(field.coverage[i]) / 255.0 * share;
                total += share;

                for (c, f) in color.iter_mut().zip(field_color) {
                    *c += f * share;
                }
            }

            let unit = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let color = color.map(|c| if total > 0.0 { c / total } else { 0.0 });
            rgba.extend(color.map(|c| unit(c.sqrt())));
            rgba.push(unit(total));
        }

        Some((resolution, rgba))
    }
}

/// Resident page assembled from persisted grass fields and streamed terrain relief.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationFieldPage {
    pub origin_xz: [f32; 2],
    pub size: f32,
    pub surface: VegetationSurfaceField,
    pub fields: Vec<GrassField>,
}

impl VegetationFieldPage {
    pub fn owns(&self, world_xz: [f32; 2]) -> bool {
        world_xz[0] >= self.origin_xz[0]
            && world_xz[1] >= self.origin_xz[1]
            && world_xz[0] < self.origin_xz[0] + self.size
            && world_xz[1] < self.origin_xz[1] + self.size
    }

    pub fn validate(&self, catalog: &VegetationCatalog) -> Result<(), PageValidationError> {
        if !self.origin_xz.into_iter().all(f32::is_finite)
            || !finite_range(self.size, 0.01, 65_536.0)
        {
            return Err(PageValidationError::InvalidExtent);
        }

        if !self.surface.is_valid() {
            return Err(PageValidationError::InvalidSurface);
        }

        self.data().validate(catalog)
    }

    pub fn data(&self) -> VegetationFieldPageData {
        VegetationFieldPageData {
            fields: self.fields.clone(),
        }
    }

    pub fn from_data(
        origin_xz: [f32; 2],
        size: f32,
        surface: VegetationSurfaceField,
        data: VegetationFieldPageData,
    ) -> Self {
        Self {
            origin_xz,
            size,
            surface,
            fields: data.fields,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum PageValidationError {
    #[error("invalid vegetation page extent")]
    InvalidExtent,
    #[error("invalid vegetation page surface field")]
    InvalidSurface,
    #[error("vegetation page references missing grass {0:?}")]
    MissingType(GrassTypeId),
    #[error("vegetation page repeats grass {0:?}")]
    DuplicateType(GrassTypeId),
    #[error("invalid field for grass {0:?}")]
    InvalidField(GrassTypeId),
}

/// Page-local terrain samples shared by every grass type on the page.
///
/// Samples include both page edges, so adjacent cooked pages can carry identical boundary values.
/// Normals use signed 16-bit octahedral encoding; validity is R8. This keeps the persistent
/// contract compact while exposing ordinary world-space samples to CPU and GPU placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationSurfaceField {
    pub resolution: u16,
    pub heights: Vec<f32>,
    pub normals_oct: Vec<[i16; 2]>,
    pub validity: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VegetationSurfaceSample {
    pub height: f32,
    pub normal: [f32; 3],
    pub validity: f32,
}

/// The corner samples (as indices, 00, 10, 01, 11) of the rendered triangle at `local`, a share
/// of the page along X and Z clamped to it, and their weights.
pub fn surface_triangle(local: [f32; 2], resolution: usize) -> ([usize; 4], [f32; 4]) {
    let grid = local.map(|v| v.clamp(0.0, 1.0) * (resolution - 1) as f32);
    let [x0, z0] = grid.map(|v| v.floor() as usize);
    let (x1, z1) = ((x0 + 1).min(resolution - 1), (z0 + 1).min(resolution - 1));
    let indices = [
        z0 * resolution + x0,
        z0 * resolution + x1,
        z1 * resolution + x0,
        z1 * resolution + x1,
    ];
    let weights = surface_triangle_weights(grid[0] - x0 as f32, grid[1] - z0 as f32);
    (indices, weights)
}

/// Weights for corners (00, 10, 01, 11), matching the terrain renderer's 00--11 diagonal.
/// Keep the GPU `sample_surface` implementation in agreement with this CPU reference.
pub fn surface_triangle_weights(x: f32, z: f32) -> [f32; 4] {
    if z >= x {
        [1.0 - z, 0.0, z - x, x]
    } else {
        [1.0 - x, x - z, 0.0, z]
    }
}

impl VegetationSurfaceField {
    pub fn flat(resolution: u16, height: f32, normal: [f32; 3]) -> Self {
        let sample_count = usize::from(resolution).saturating_mul(usize::from(resolution));
        Self {
            resolution,
            heights: vec![height; sample_count],
            normals_oct: vec![encode_octahedral_normal(normal); sample_count],
            validity: vec![u8::MAX; sample_count],
        }
    }

    pub fn sample(
        &self,
        page_origin_xz: [f32; 2],
        page_size: f32,
        world_xz: [f32; 2],
    ) -> VegetationSurfaceSample {
        let resolution = usize::from(self.resolution);

        // Scene admission validates every height once. Sampling must remain O(1),
        // including debug builds used by the editor.
        debug_assert!((2..=257).contains(&resolution));
        debug_assert_eq!(self.heights.len(), resolution * resolution);
        debug_assert_eq!(self.normals_oct.len(), resolution * resolution);
        debug_assert_eq!(self.validity.len(), resolution * resolution);
        let local = [
            (world_xz[0] - page_origin_xz[0]) / page_size,
            (world_xz[1] - page_origin_xz[1]) / page_size,
        ];
        let (indices, weights) = surface_triangle(local, resolution);
        let mut height = 0.0;
        let mut normal = [0.0; 3];
        let mut validity = 0.0;

        for (index, weight) in indices.into_iter().zip(weights) {
            height += self.heights[index] * weight;
            let decoded = decode_octahedral_normal(self.normals_oct[index]);
            normal[0] += decoded[0] * weight;
            normal[1] += decoded[1] * weight;
            normal[2] += decoded[2] * weight;
            validity += f32::from(self.validity[index]) / 255.0 * weight;
        }

        VegetationSurfaceSample {
            height,
            normal: normalize3_or(normal, [0.0, 1.0, 0.0]),
            validity,
        }
    }

    fn is_valid(&self) -> bool {
        let resolution = usize::from(self.resolution);
        let expected = resolution.saturating_mul(resolution);
        (2..=257).contains(&resolution)
            && self.heights.len() == expected
            && self.normals_oct.len() == expected
            && self.validity.len() == expected
            && self.heights.iter().all(|height| height.is_finite())
    }
}

pub fn encode_octahedral_normal(normal: [f32; 3]) -> [i16; 2] {
    encode_octahedral_normal_bits(normal, 16)
}

/// [`encode_octahedral_normal`] keeping `bits` of each axis; the stored `i16`'s low bits are
/// zero.
pub fn encode_octahedral_normal_bits(normal: [f32; 3], bits: u32) -> [i16; 2] {
    let unit = ((1_i32 << (bits - 1)) - 1) as f32;
    let shift = 16 - bits;
    let normal = normalize3_or(normal, [0.0, 1.0, 0.0]);
    let inverse_l1 = (normal[0].abs() + normal[1].abs() + normal[2].abs()).recip();
    let mut encoded = [normal[0] * inverse_l1, normal[2] * inverse_l1];

    if normal[1] < 0.0 {
        encoded = [
            (1.0 - encoded[1].abs()) * encoded[0].signum(),
            (1.0 - encoded[0].abs()) * encoded[1].signum(),
        ];
    }

    [
        ((encoded[0].clamp(-1.0, 1.0) * unit).round() as i16) << shift,
        ((encoded[1].clamp(-1.0, 1.0) * unit).round() as i16) << shift,
    ]
}

pub fn decode_octahedral_normal(encoded: [i16; 2]) -> [f32; 3] {
    let encoded = [
        f32::from(encoded[0]) / f32::from(i16::MAX),
        f32::from(encoded[1]) / f32::from(i16::MAX),
    ];
    let mut normal = [
        encoded[0],
        1.0 - encoded[0].abs() - encoded[1].abs(),
        encoded[1],
    ];

    if normal[1] < 0.0 {
        let original_x = normal[0];
        normal[0] = (1.0 - normal[2].abs()) * original_x.signum();
        normal[2] = (1.0 - original_x.abs()) * normal[2].signum();
    }

    normalize3_or(normal, [0.0, 1.0, 0.0])
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VegetationScene {
    pub catalog: VegetationCatalog,
    pub pages: Vec<VegetationFieldPage>,
}

impl VegetationScene {
    pub fn validate(&self) -> Result<(), SceneValidationError> {
        self.catalog.validate()?;

        for (index, page) in self.pages.iter().enumerate() {
            page.validate(&self.catalog)
                .map_err(|source| SceneValidationError::Page { index, source })?;
        }

        for (first, first_page) in self.pages.iter().enumerate() {
            for (second, second_page) in self.pages.iter().enumerate().skip(first + 1) {
                if pages_overlap(first_page, second_page) {
                    return Err(SceneValidationError::OverlappingPages { first, second });
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SceneValidationError {
    #[error(transparent)]
    Catalog(#[from] CatalogValidationError),
    #[error("invalid vegetation page {index}: {source}")]
    Page {
        index: usize,
        source: PageValidationError,
    },
    #[error("vegetation pages {first} and {second} overlap")]
    OverlappingPages { first: usize, second: usize },
}

fn pages_overlap(first: &VegetationFieldPage, second: &VegetationFieldPage) -> bool {
    first.origin_xz[0] < second.origin_xz[0] + second.size
        && first.origin_xz[0] + first.size > second.origin_xz[0]
        && first.origin_xz[1] < second.origin_xz[1] + second.size
        && first.origin_xz[1] + first.size > second.origin_xz[1]
}

pub fn normalize3_or(value: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let length_squared = value[0] * value[0] + value[1] * value[1] + value[2] * value[2];

    if length_squared <= 1e-10 {
        return fallback;
    }

    let inverse_length = length_squared.sqrt().recip();
    [
        value[0] * inverse_length,
        value[1] * inverse_length,
        value[2] * inverse_length,
    ]
}

fn finite_range(value: f32, minimum: f32, maximum: f32) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_fixture_is_valid() {
        fixtures::reference_scene().validate().unwrap();
    }

    #[test]
    fn type_ids_follow_keys() {
        let meadow = fixtures::meadow();
        assert_eq!(meadow.id(), GrassTypeId::from_key("meadow"));
        assert_ne!(meadow.id(), fixtures::tufts().id());

        // Pinned: cooked fields name types by these bytes, and placement seeds from them.
        assert_eq!(
            GrassTypeId::from_key("meadow").0,
            [
                32, 178, 120, 38, 118, 198, 84, 235, 243, 133, 212, 231, 125, 158, 138, 93
            ]
        );
        assert_eq!(GrassTypeId::from_key("meadow").seed(), 645_444_128);
    }

    #[test]
    fn catalog_rejects_out_of_range_types_and_unknown_surface_grass() {
        let mut catalog = fixtures::reference_catalog();
        catalog.grass[0].height = [0.8, 0.4];
        assert!(matches!(
            catalog.validate(),
            Err(CatalogValidationError::InvalidType(_))
        ));
        let mut catalog = fixtures::reference_catalog();
        catalog.surfaces.push(SurfaceGrass {
            surface: [9; 16],
            grass: vec![GrassAmount {
                grass: GrassTypeId::from_key("nonexistent"),
                amount: 1.0,
            }],
        });
        assert!(matches!(
            catalog.validate(),
            Err(CatalogValidationError::InvalidSurface(_))
        ));
        let mut catalog = fixtures::reference_catalog();
        catalog.grass.push(fixtures::meadow());
        assert!(matches!(
            catalog.validate(),
            Err(CatalogValidationError::DuplicateType(_))
        ));
    }

    #[test]
    fn wear_bands_rise_peak_and_fall() {
        let band = [0.1, 0.3, 0.6];
        let near = |wear: f64, share: f64| (wear_share(band, wear) - share).abs() < 1e-5;
        assert!(near(0.0, 0.0) && near(0.1, 0.0));
        assert!(near(0.2, 0.5) && near(0.3, 1.0) && near(0.45, 0.5));
        assert!(near(0.6, 0.0) && near(1.0, 0.0));

        // A band ending at its peak stays full above it.
        assert_eq!(wear_share([0.5, 0.8, 0.8], 1.0), 1.0);
    }

    #[test]
    fn packs_resolve_surface_and_grass_keys() {
        let pebbles = DetailType {
            key: "pebbles".into(),
            assets: vec!["pebbles.glb".into()],
            density: 1.0,
            scale: [1.0, 1.5],
            clumps: DetailClumps {
                size: 1.0,
                pull: 0.5,
                patches: 0.5,
            },
            sink: 0.3,
            align: 0.5,
            tilt: 0.2,
            distance: 30.0,
            shadows: false,
            on_paths: true,
            wear: None,
        };
        let pack = GrassPack {
            terrain: "test".into(),
            types: vec![fixtures::meadow(), fixtures::tufts()],
            details: vec![pebbles],
            surfaces: [
                ("lawn".to_string(), vec![("meadow".to_string(), 0.5)]),
                ("path".to_string(), vec![("pebbles".to_string(), 0.8)]),
            ]
            .into(),
        };
        let path = pack
            .catalog(|key| match key {
                "lawn" => Some([7; 16]),
                "path" => Some([9; 16]),
                _ => None,
            })
            .unwrap();

        // Details share the grass types' ID space; grass cover ignores them.
        let on_path = path.on_surface([9; 16]);
        assert_eq!(on_path.len(), 1);
        assert!(path.detail(on_path[0].grass).is_some());
        assert_eq!(path.cover([9; 16]).amount, 0.0);
        let catalog = path;
        assert_eq!(
            catalog.on_surface([7; 16]),
            &[GrassAmount {
                grass: GrassTypeId::from_key("meadow"),
                amount: 0.5
            }]
        );
        assert!(catalog.on_surface([8; 16]).is_empty());
        assert!(pack.catalog(|_| None).is_err());
        let mut unknown = pack.clone();
        unknown
            .surfaces
            .insert("lawn".into(), vec![("clover".into(), 1.0)]);
        assert!(unknown.catalog(|_| Some([7; 16])).is_err());
    }

    #[test]
    fn live_looks_keep_the_cooked_set_of_types() {
        let mut catalog = fixtures::reference_catalog();
        let mut edited = catalog.clone();
        edited.grass[0].colors.tip = [0.5, 0.5, 0.1];
        edited.grass.push(GrassType {
            key: "new".into(),
            ..fixtures::meadow()
        });
        assert!(catalog.adopt_looks(&edited));
        assert_eq!(catalog.grass.len(), 2);
        assert_eq!(catalog.grass[0].colors.tip, [0.5, 0.5, 0.1]);
        assert!(!catalog.adopt_looks(&edited));
    }

    #[test]
    fn cover_mixes_field_colours_and_tufted_grass_covers_less() {
        let mut catalog = fixtures::reference_catalog();
        let [meadow, tufts] = [0, 1].map(|i| catalog.grass[i].id());
        let surface = |grass: Vec<(GrassTypeId, f32)>, key: u8| SurfaceGrass {
            surface: [key; 16],
            grass: grass
                .into_iter()
                .map(|(grass, amount)| GrassAmount { grass, amount })
                .collect(),
        };
        catalog.surfaces = vec![
            surface(vec![(meadow, 1.0)], 1),
            surface(vec![(tufts, 1.0)], 2),
            surface(vec![(meadow, 1.0), (tufts, 1.0)], 3),
        ];
        assert_eq!(catalog.cover([9; 16]), GrassCover::default());
        let even = catalog.cover([1; 16]);
        let tufted = catalog.cover([2; 16]);
        assert_eq!(even.color, catalog.grass[0].colors.field());
        assert!(tufted.amount < even.amount);
        let tufts = catalog.grass[1].clone();
        assert!(tufts.twin);
        assert_eq!(tufts.blades_per_square_metre(), 2.0 * tufts.density);
        let both = catalog.cover([3; 16]);
        assert!(both.amount <= 1.0 && both.amount > even.amount);

        for c in 0..3 {
            let (a, b) = (even.color[c], tufted.color[c]);
            assert!(both.color[c] >= a.min(b) && both.color[c] <= a.max(b));
        }
    }

    #[test]
    fn cover_map_follows_where_grass_grows() {
        let catalog = fixtures::reference_catalog();
        let [meadow, tufts] = [0, 1].map(|i| catalog.grass[i].id());
        let field = |grass, coverage: [u8; 4]| GrassField {
            grass,
            resolution: 2,
            coverage: coverage.to_vec(),
        };

        // Meadow everywhere but a worn texel; tufts in one corner.
        let data = VegetationFieldPageData {
            fields: vec![
                field(meadow, [255, 255, 255, 0]),
                field(tufts, [0, 255, 0, 0]),
            ],
        };
        let (resolution, rgba) = data.cover_map(&catalog).unwrap();
        assert_eq!((resolution, rgba.len()), (2, 16));
        let share = VegetationCatalog::cover_share(&catalog.grass[0]);
        assert_eq!(rgba[3], (share * 255.0).round() as u8);
        assert_eq!(rgba[15], 0);
        let tufted = (share + VegetationCatalog::cover_share(&catalog.grass[1])).min(1.0);
        assert_eq!(rgba[7], (tufted * 255.0).round() as u8);
        let field_color = catalog.grass[0].colors.field();

        for c in 0..3 {
            let decoded = (f32::from(rgba[c]) / 255.0).powi(2);
            assert!((decoded - field_color[c]).abs() < 0.01);
        }

        assert!(
            VegetationFieldPageData { fields: vec![] }
                .cover_map(&catalog)
                .is_none()
        );
    }

    #[test]
    fn overlapping_pages_are_rejected_before_they_duplicate_roots() {
        let mut scene = fixtures::reference_scene();
        scene.pages[1].origin_xz = [15.99, 0.0];
        assert!(matches!(
            scene.validate(),
            Err(SceneValidationError::OverlappingPages {
                first: 0,
                second: 1
            })
        ));
    }

    #[test]
    fn surface_sampling_interpolates_height_and_decodes_normals() {
        let normal = normalize3_or([0.25, 1.0, -0.4], [0.0, 1.0, 0.0]);
        let surface = VegetationSurfaceField {
            resolution: 2,
            heights: vec![0.0, 2.0, 4.0, 6.0],
            normals_oct: vec![encode_octahedral_normal(normal); 4],
            validity: vec![u8::MAX; 4],
        };
        let sample = surface.sample([0.0, 0.0], 8.0, [4.0, 4.0]);
        assert!((sample.height - 3.0).abs() < 1e-6);
        assert!((sample.normal[0] - normal[0]).abs() < 1e-4);
        assert!((sample.normal[1] - normal[1]).abs() < 1e-4);
        assert!((sample.normal[2] - normal[2]).abs() < 1e-4);
        assert_eq!(sample.validity, 1.0);
    }
}
