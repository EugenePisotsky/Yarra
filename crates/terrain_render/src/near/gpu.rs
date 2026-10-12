use super::*;
use bevy::{
    asset::AssetId,
    core_pipeline::schedule::camera_driver,
    render::{
        RenderApp,
        render_asset::RenderAssets,
        render_resource::*,
        renderer::{RenderGraph, RenderQueue},
        storage::GpuShaderBuffer,
        texture::GpuImage,
    },
};
use std::sync::{Arc, Mutex, Weak};
use world::MAX_TERRAIN_SURFACES_PER_CELL;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pack {
    pub base: Handle<Image>,
    pub normal: Handle<Image>,
    pub macro_image: Handle<Image>,
}
impl Pack {
    pub(crate) fn from_material(m: &TerrainMaterial) -> Self {
        Self {
            base: m.base_color_array.clone(),
            normal: m.normal_material_array.clone(),
            macro_image: m.macro_variation.clone(),
        }
    }
    pub fn matches(&self, m: &TerrainMaterial) -> bool {
        self.base == m.base_color_array
            && self.normal == m.normal_material_array
            && self.macro_image == m.macro_variation
    }
}
/// One of a page's surfaces, as `terrain_near.wesl` reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub(crate) struct NearSlot {
    /// Array layer, metres per repetition, anti-tiling (0/1), normal strength.
    pub surface: Vec4,
    /// Normal Y sign, roughness minimum and maximum; W unused.
    pub material: Vec4,
    pub phase: Vec4,
    pub lattice: Vec4,
    /// The grass field's linear colour and how fully grass covers the surface.
    pub grass: Vec4,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub(crate) struct NearEntry {
    pub key: IVec4,
    /// The page's roads in the road buffer: first tile and tiles across (`road_records`).
    pub roads: IVec4,
    /// Fade, weight resolution, surface count.
    pub state: Vec4,
    pub macro_scales: Vec4,
    pub macro_settings: Vec4,
    pub macro0: Vec4,
    pub macro1: Vec4,
    pub macro2: Vec4,
    pub slots: [NearSlot; MAX_TERRAIN_SURFACES_PER_CELL],
}
pub(super) fn bytes() -> u64 {
    // Two RGBA weight layers and a grass cover layer per slot.
    u64::from(WEIGHT_SIDE * WEIGHT_SIDE * 8 + GRASS_SIDE * GRASS_SIDE * 4) * NEAR_SLOTS as u64
        + NearEntry::min_size().get() * NEAR_TABLE as u64
        + road_buffer_bytes()
}
/// Largest grass cover (`NearGrassCover`) the grass atlas holds: a 32 m cell's half-metre
/// grass fields.
pub(super) const GRASS_SIDE: u32 = 64;
/// Each slot's region of the road buffer, in vec4s: about 250 centre-line segments, several
/// roads' worth through one cell.
pub(super) const ROAD_CAPACITY: u32 = 1024;
/// Tiles across a page's road grid: on 32 m cells, 2 m tiles.
pub(super) const ROAD_TILES: usize = 16;
fn road_buffer_bytes() -> u64 {
    u64::from(ROAD_CAPACITY) * NEAR_SLOTS as u64 * 16
}
/// A page's roads as `terrain_near.wesl` reads them from its slot's region of the road buffer,
/// which starts at vec4 `base`: six vec4 per road (`terrain_road.wesl` lists their fields), two
/// per junction, then the page's 16 x 16 tiles, one vec4 each (rows of increasing X from the
/// page's origin), then three vec4 per segment (ends; end normals; end widths and road). A tile
/// holds two runs of segments, first and count each, together every segment that reaches the
/// tile, so a pixel tests only those; its first count is negative where they belong to more than
/// one road. Indices are absolute vec4 indices. Roads whose ground has no surface on the page
/// draw nothing. Returns the records and the entry's `roads` (first tile, tiles across, first
/// junction, junctions), or `None` when they do not fit the region.
pub(super) fn road_records(s: &NearSource, base: u32) -> Option<(Vec<Vec4>, IVec4)> {
    let roads = &s.roads;
    if roads.segments.is_empty() {
        return Some((vec![], IVec4::ZERO));
    }
    let first_junction = base as usize + roads.roads.len() * 6;
    let first_tile = first_junction + roads.junctions.len() * 2;
    let first_segment = first_tile + ROAD_TILES * ROAD_TILES;
    let end = first_segment + roads.segments.len() * 3;
    if end > (base + ROAD_CAPACITY) as usize {
        return None;
    }
    let mut records = Vec::with_capacity(end - base as usize);
    for road in &roads.roads {
        let mut ground: Vec<_> = road
            .surfaces
            .iter()
            .filter_map(|(id, w)| Some((s.surfaces.iter().position(|s| s == id)? as f32, *w)))
            .collect();
        ground.sort_by(|a, b| b.1.total_cmp(&a.1));
        ground.resize(4, (-1., 0.));
        let shape = &road.shape;
        records.extend([
            Vec4::new(
                shape.track_spacing,
                shape.track_width,
                shape.edge_softness,
                shape.edge_variation,
            ),
            Vec4::new(
                shape.edge_patch_size,
                shape.breakup_patch_size,
                (shape.seed & 0xffff) as f32,
                (shape.seed >> 16) as f32,
            ),
            Vec4::new(
                shape.breakup,
                road.center_ground,
                road.shoulder_ground,
                road.strength,
            ),
            Vec4::new(
                road.track_depth,
                road.rut_roundness,
                shape.track_wander,
                shape.track_wander_length,
            ),
            Vec4::from_array(std::array::from_fn(|i| ground[i].0)),
            Vec4::from_array(std::array::from_fn(|i| ground[i].1)),
        ]);
    }
    // Junctions take their road's ground; a page road's bit marks each road that meets there.
    for j in &roads.junctions {
        let members = j.members & 0x00ff_ffff;
        records.extend([
            Vec4::new(j.centre[0], j.centre[1], j.radius, (j.seed & 0xffff) as f32),
            Vec4::new(
                (j.seed >> 16) as f32,
                (base as usize + usize::from(j.road) * 6) as f32,
                members as f32,
                base as f32,
            ),
        ]);
    }
    // How far each segment's drawing reaches. Bilinear weights carry the baked road a texel and
    // a half beyond it, and the shader redraws the ground that far.
    let texel = s
        .weights
        .first()
        .map_or(0., |w| s.cell_size / f32::from(w.resolution.max(2) - 1));
    let bounds: Vec<_> = roads
        .segments
        .iter()
        .map(|segment| {
            let shape = &roads.roads[usize::from(segment.road)].shape;
            let pad = segment.width[0].max(segment.width[1]) * 0.5
                + shape.edge_variation
                + shape.edge_softness
                + texel * 1.5;
            let (a, b) = (Vec2::from_array(segment.a), Vec2::from_array(segment.b));
            (a.min(b) - pad, a.max(b) + pad)
        })
        .collect();
    let size = s.cell_size / ROAD_TILES as f32;
    for z in 0..ROAD_TILES {
        for x in 0..ROAD_TILES {
            let lo = Vec2::new(x as f32, z as f32) * size;
            let hi = lo + size;
            // Runs of consecutive segments reaching the tile; the closest runs merge until two
            // remain, which may add segments that do not reach it, never drop one that does.
            let mut runs: Vec<std::ops::Range<usize>> = vec![];
            for (i, (a, b)) in bounds.iter().enumerate() {
                if a.cmple(hi).all() && b.cmpge(lo).all() {
                    match runs.last_mut() {
                        Some(r) if r.end == i => r.end = i + 1,
                        _ => runs.push(i..i + 1),
                    }
                }
            }
            while runs.len() > 2 {
                let i = (1..runs.len())
                    .min_by_key(|&i| runs[i].start - runs[i - 1].end)
                    .unwrap();
                runs[i - 1].end = runs[i].end;
                runs.remove(i);
            }
            let run = |i: usize| {
                runs.get(i).map_or(Vec2::ZERO, |r| {
                    Vec2::new((first_segment + r.start * 3) as f32, r.len() as f32)
                })
            };
            let (first, second) = (run(0), run(1));
            let mut owners = runs
                .iter()
                .flat_map(|r| r.clone())
                .map(|i| roads.segments[i].road);
            let mixed = owners
                .next()
                .is_some_and(|road| owners.any(|other| other != road));
            let count = if mixed { -first.y } else { first.y };
            records.push(Vec4::new(first.x, count, second.x, second.y));
        }
    }
    for segment in &roads.segments {
        records.extend([
            Vec4::new(segment.a[0], segment.a[1], segment.b[0], segment.b[1]),
            Vec4::new(
                segment.normal_a[0],
                segment.normal_a[1],
                segment.normal_b[0],
                segment.normal_b[1],
            ),
            Vec4::new(
                segment.width[0],
                segment.width[1],
                (base as usize + usize::from(segment.road) * 6) as f32,
                0.,
            ),
        ]);
    }
    Some((
        records,
        IVec4::new(
            first_tile as i32,
            ROAD_TILES as i32,
            first_junction as i32,
            roads.junctions.len() as i32,
        ),
    ))
}
fn phase(v: [f64; 2]) -> Vec2 {
    Vec2::new(v[0].rem_euclid(1.) as f32, v[1].rem_euclid(1.) as f32)
}
fn transformed(p: [f64; 2]) -> [f64; 2] {
    [p[0] + p[1] * 0.5773502692, p[1] * 1.1547005384]
}
/// UV phase (XY) and the anti-tiling lattice cell and phase of a page's origin `p`.
fn uv_phase(p: [f64; 2], size: f32) -> (Vec4, Vec4) {
    let uv = p.map(|v| v / f64::from(size.max(0.001)));
    let lattice = transformed(uv);
    let a = phase(uv);
    let f = phase(lattice);
    (
        Vec4::new(a.x, a.y, 0., 0.),
        Vec4::new(
            lattice[0].floor() as f32,
            lattice[1].floor() as f32,
            f.x,
            f.y,
        ),
    )
}
pub(super) fn entry(
    s: &NearSource,
    m: &TerrainMaterial,
    slot: u32,
    fade: f32,
    roads: IVec4,
    cover_side: u32,
) -> NearEntry {
    let settings = m.settings;
    let p = s.key.cell.origin(s.cell_size);
    let mut slots = [NearSlot::default(); MAX_TERRAIN_SURFACES_PER_CELL];
    for (slot, layer) in slots.iter_mut().zip(&s.layers) {
        let surface = &layer.surface;
        let (phase, lattice) = uv_phase(p, surface.tile_size);
        *slot = NearSlot {
            surface: Vec4::new(
                f32::from(layer.layer),
                surface.tile_size,
                f32::from(surface.anti_tiling),
                surface.normal_strength,
            ),
            material: Vec4::new(
                surface.normal_y_sign,
                surface.roughness_min,
                surface.roughness_max,
                0.,
            ),
            phase,
            lattice,
            grass: Vec4::from_array(layer.grass),
        };
    }
    let macro_phase = |i, p: [f64; 2]| {
        phase(p.map(|v| v / f64::from(settings.macro_scales[i])))
            .extend(0.)
            .extend(0.)
    };
    NearEntry {
        key: IVec4::new(s.key.cell.x, s.key.cell.z, 0, slot as i32 + 1),
        roads,
        state: Vec4::new(
            fade,
            if s.surfaces.len() == 1 {
                1.
            } else {
                s.weights[0].resolution as f32
            },
            s.surfaces.len() as f32,
            cover_side as f32,
        ),
        macro_scales: settings.macro_scales,
        macro_settings: settings.macro_settings,
        macro0: macro_phase(0, p),
        macro1: macro_phase(
            1,
            [p[0] * 0.819 - p[1] * 0.574, p[0] * 0.574 + p[1] * 0.819],
        ),
        macro2: macro_phase(
            2,
            [p[0] * -0.342 - p[1] * 0.940, p[0] * 0.940 + p[1] * -0.342],
        ),
        slots,
    }
}
pub(super) struct Tile {
    slot: u32,
    /// Two RGBA layers of surface weights, four surfaces each.
    pub(super) weights: [Vec<u8>; 2],
    weight_side: u32,
    /// The grass cover layer (`NearGrassCover`), empty for none.
    cover: Vec<u8>,
    pub(super) cover_side: u32,
    /// The page's road records (`road_records`), as bytes, and the entry's `roads`.
    roads: Vec<u8>,
    pub(super) road_entry: IVec4,
}
/// The page's upload; `None` roads draw only as the page's weights carry them.
pub(super) fn tile(slot: u32, s: &NearSource, cover: Option<&NearGrassCover>) -> (Tile, bool) {
    let (weight_side, weights) = if s.surfaces.len() == 1 {
        (1, [vec![255, 0, 0, 0], vec![0; 4]])
    } else {
        let side = s.weights[0].resolution as u32;
        let page = |i: usize| {
            s.weights
                .get(i)
                .map_or_else(|| vec![0; (side * side * 4) as usize], |w| w.rgba.clone())
        };
        (side, [page(0), page(1)])
    };
    let road = road_records(s, slot * ROAD_CAPACITY);
    let fits = road.is_some();
    let (roads, road_entry) = road.unwrap_or_default();
    let roads = if roads.is_empty() {
        vec![]
    } else {
        crate::storage_bytes(&roads)
    };
    let cover = cover.filter(|c| {
        c.resolution > 0
            && u32::from(c.resolution) <= GRASS_SIDE
            && c.rgba.len() == usize::from(c.resolution).pow(2) * 4
    });
    (
        Tile {
            slot,
            weights,
            weight_side,
            cover: cover.map_or(vec![], |c| c.rgba.clone()),
            cover_side: cover.map_or(0, |c| u32::from(c.resolution)),
            roads,
            road_entry,
        },
        fits,
    )
}
struct Update {
    table: Vec<u8>,
    tiles: Vec<Tile>,
}
struct Shared {
    weights: AssetId<Image>,
    grass: AssetId<Image>,
    table: AssetId<ShaderBuffer>,
    roads: AssetId<ShaderBuffer>,
    pack: Pack,
    pending: Option<Update>,
    ready: bool,
    uploads: u64,
}
#[derive(Resource, Default, Clone)]
pub(super) struct NearUploadHub(Arc<Mutex<Vec<Weak<Mutex<Shared>>>>>);
pub(crate) struct NearAtlas {
    pub weights: Handle<Image>,
    pub grass: Handle<Image>,
    pub table: Handle<ShaderBuffer>,
    pub roads: Handle<ShaderBuffer>,
    pub(crate) pack: Pack,
    shared: Arc<Mutex<Shared>>,
}
impl NearAtlas {
    pub(super) fn new(
        images: &mut Assets<Image>,
        buffers: &mut Assets<ShaderBuffer>,
        hub: &NearUploadHub,
        pack: Pack,
    ) -> Self {
        let image = |side, format, layers| {
            let mut image = Image::new_uninit(
                Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: layers,
                },
                TextureDimension::D2,
                format,
                RenderAssetUsages::RENDER_WORLD,
            );
            image.texture_view_descriptor = Some(TextureViewDescriptor {
                dimension: Some(TextureViewDimension::D2Array),
                ..default()
            });
            image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                ..default()
            });
            image
        };
        let slots = NEAR_SLOTS as u32;
        let weights = images.add(image(WEIGHT_SIDE, TextureFormat::Rgba8Unorm, slots * 2));
        let grass = images.add(image(GRASS_SIDE, TextureFormat::Rgba8Unorm, slots));
        let table = buffers.add(ShaderBuffer::with_size(
            NearEntry::min_size().get() * NEAR_TABLE as u64,
            RenderAssetUsages::RENDER_WORLD,
        ));
        let roads = buffers.add(ShaderBuffer::with_size(
            road_buffer_bytes(),
            RenderAssetUsages::RENDER_WORLD,
        ));
        let shared = Arc::new(Mutex::new(Shared {
            weights: weights.id(),
            grass: grass.id(),
            table: table.id(),
            roads: roads.id(),
            pack: pack.clone(),
            pending: None,
            ready: false,
            uploads: 0,
        }));
        hub.0.lock().unwrap().push(Arc::downgrade(&shared));
        let atlas = Self {
            weights,
            grass,
            table,
            roads,
            pack,
            shared,
        };
        atlas.submit(vec![NearEntry::default(); NEAR_TABLE], vec![]);
        atlas
    }
    pub fn ready(&self) -> bool {
        self.shared.lock().unwrap().ready
    }
    pub fn idle(&self) -> bool {
        self.shared.lock().unwrap().pending.is_none()
    }
    pub fn uploads(&self) -> u64 {
        self.shared.lock().unwrap().uploads
    }
    pub(super) fn submit(&self, table: Vec<NearEntry>, tiles: Vec<Tile>) {
        assert_eq!(table.len(), NEAR_TABLE);
        assert!(tiles.len() <= 2);
        let mut s = self.shared.lock().unwrap();
        assert!(s.pending.is_none());
        s.pending = Some(Update {
            table: crate::storage_bytes(&table),
            tiles,
        });
    }
}
pub(super) fn install(app: &mut App) {
    let hub = NearUploadHub::default();
    app.insert_resource(hub.clone());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .insert_resource(hub)
            .add_systems(RenderGraph, upload.before(camera_driver));
    }
}
fn upload(
    hub: Res<NearUploadHub>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
) {
    hub.0.lock().unwrap().retain(|weak| {
        let Some(shared) = weak.upgrade() else {
            return false;
        };
        let mut s = shared.lock().unwrap();
        let (Some(weights), Some(grass), Some(table), Some(roads)) = (
            images.get(s.weights),
            images.get(s.grass),
            buffers.get(s.table),
            buffers.get(s.roads),
        ) else {
            return true;
        };
        if [&s.pack.base, &s.pack.normal, &s.pack.macro_image]
            .into_iter()
            .any(|h| images.get(h).is_none())
        {
            return true;
        }
        let Some(update) = s.pending.take() else {
            return true;
        };
        for t in &update.tiles {
            let layers = [
                (weights, t.weight_side, t.slot * 2, &t.weights[0]),
                (weights, t.weight_side, t.slot * 2 + 1, &t.weights[1]),
                (grass, t.cover_side, t.slot, &t.cover),
            ];
            for (image, side, layer, data) in layers {
                if data.is_empty() {
                    continue;
                }
                queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: &image.texture,
                        mip_level: 0,
                        origin: Origin3d {
                            x: 0,
                            y: 0,
                            z: layer,
                        },
                        aspect: TextureAspect::All,
                    },
                    data,
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(side * 4),
                        rows_per_image: Some(side),
                    },
                    Extent3d {
                        width: side,
                        height: side,
                        depth_or_array_layers: 1,
                    },
                );
            }
            if !t.roads.is_empty() {
                queue.write_buffer(
                    &roads.buffer,
                    u64::from(t.slot * ROAD_CAPACITY) * 16,
                    &t.roads,
                );
            }
        }
        queue.write_buffer(&table.buffer, 0, &update.table);
        s.uploads += update.tiles.len() as u64;
        s.ready = true;
        true
    });
}
