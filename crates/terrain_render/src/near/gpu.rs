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
    /// Normal Y sign, roughness minimum and maximum, and the page slot light wear tramples the
    /// surface to (-1: none).
    pub material: Vec4,
    pub phase: Vec4,
    pub lattice: Vec4,
    /// The grass field's linear colour and how fully grass covers the surface.
    pub grass: Vec4,
    /// The page slots the surface wears down to (-1: none) and their shares
    /// (`world::TerrainWear::worn`).
    pub worn: Vec4,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub(crate) struct NearEntry {
    pub key: IVec4,
    /// X: whether the page has a wear field, in its slot's layer of the wear atlas.
    pub wear: Vec4,
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
    // The RGBA weight layers and a grass cover layer per slot.
    u64::from(WEIGHT_SIDE * WEIGHT_SIDE * 4 * WEIGHT_LAYERS as u32 + GRASS_SIDE * GRASS_SIDE * 4)
        * NEAR_SLOTS as u64
        + NearEntry::min_size().get() * NEAR_TABLE as u64
        + u64::from(WEAR_SIDE * WEAR_SIDE * 4) * NEAR_SLOTS as u64
}
/// RGBA weight layers per slot: four surfaces each, `MAX_TERRAIN_SURFACES_PER_CELL` in all.
pub(super) const WEIGHT_LAYERS: usize = MAX_TERRAIN_SURFACES_PER_CELL.div_ceil(4);
/// Largest grass cover (`NearGrassCover`) the grass atlas holds: a 32 m cell's half-metre
/// grass fields.
pub(super) const GRASS_SIDE: u32 = 64;
/// Texels per side of a page's wear field (`world::TerrainWear`), as the wear atlas holds it.
pub(super) const WEAR_SIDE: u32 = world::WEAR_RESOLUTION as u32;
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
    wear: bool,
    cover_side: u32,
) -> NearEntry {
    let settings = m.settings;
    let p = s.key.cell.origin(s.cell_size);
    let mut slots = [NearSlot::default(); MAX_TERRAIN_SURFACES_PER_CELL];
    for (index, (slot, layer)) in slots.iter_mut().zip(&s.layers).enumerate() {
        let surface = &layer.surface;
        let field = s.wear.as_ref().filter(|_| wear);
        let worn = field.map_or([(world::NO_WORN_SLOT, 0); 2], |w| w.worn[index]);
        let trampled = field
            .map(|w| w.trampled[index])
            .filter(|&t| t != world::NO_WORN_SLOT)
            .map_or(-1., f32::from);
        let to = |(slot, share): (u8, u8)| {
            if slot == world::NO_WORN_SLOT {
                (-1., 0.)
            } else {
                (f32::from(slot), f32::from(share) / 255.)
            }
        };
        let (a, b) = (to(worn[0]), to(worn[1]));
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
                trampled,
            ),
            phase,
            lattice,
            grass: Vec4::from_array(layer.grass),
            worn: Vec4::new(a.0, a.1, b.0, b.1),
        };
    }
    let macro_phase = |i, p: [f64; 2]| {
        phase(p.map(|v| v / f64::from(settings.macro_scales[i])))
            .extend(0.)
            .extend(0.)
    };
    NearEntry {
        key: IVec4::new(s.key.cell.x, s.key.cell.z, 0, slot as i32 + 1),
        wear: Vec4::new(f32::from(u8::from(wear)), 0., 0., 0.),
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
    /// The RGBA layers of surface weights, four surfaces each; empty past the page's surfaces.
    pub(super) weights: [Vec<u8>; WEIGHT_LAYERS],
    weight_side: u32,
    /// The grass cover layer (`NearGrassCover`), empty for none.
    cover: Vec<u8>,
    pub(super) cover_side: u32,
    /// The page's wear field (`world::TerrainWear` texels), empty for none.
    wear: Vec<u8>,
}
impl Tile {
    pub(super) fn has_wear(&self) -> bool {
        !self.wear.is_empty()
    }
}
/// The page's upload.
pub(super) fn tile(slot: u32, s: &NearSource, cover: Option<&NearGrassCover>) -> Tile {
    // The shader reads only the layers the page's surfaces fill.
    let (weight_side, weights) = if s.surfaces.len() == 1 {
        (
            1,
            std::array::from_fn(|i| if i == 0 { vec![255, 0, 0, 0] } else { vec![] }),
        )
    } else {
        let side = s.weights[0].resolution as u32;
        (
            side,
            std::array::from_fn(|i| s.weights.get(i).map_or(vec![], |w| w.rgba.clone())),
        )
    };
    let wear = s
        .wear
        .as_ref()
        .filter(|w| u32::from(w.resolution) == WEAR_SIDE && w.is_valid(s.surfaces.len()))
        .map_or(vec![], |w| w.texels.concat());
    let cover = cover.filter(|c| {
        c.resolution > 0
            && u32::from(c.resolution) <= GRASS_SIDE
            && c.rgba.len() == usize::from(c.resolution).pow(2) * 4
    });
    Tile {
        slot,
        weights,
        weight_side,
        cover: cover.map_or(vec![], |c| c.rgba.clone()),
        cover_side: cover.map_or(0, |c| u32::from(c.resolution)),
        wear,
    }
}
struct Update {
    table: Vec<u8>,
    tiles: Vec<Tile>,
}
struct Shared {
    weights: AssetId<Image>,
    grass: AssetId<Image>,
    table: AssetId<ShaderBuffer>,
    wear: AssetId<Image>,
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
    pub wear: Handle<Image>,
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
        let weights = images.add(image(
            WEIGHT_SIDE,
            TextureFormat::Rgba8Unorm,
            slots * WEIGHT_LAYERS as u32,
        ));
        let grass = images.add(image(GRASS_SIDE, TextureFormat::Rgba8Unorm, slots));
        let table = buffers.add(ShaderBuffer::with_size(
            NearEntry::min_size().get() * NEAR_TABLE as u64,
            RenderAssetUsages::RENDER_WORLD,
        ));
        let wear = images.add(image(WEAR_SIDE, TextureFormat::Rgba8Unorm, slots));
        let shared = Arc::new(Mutex::new(Shared {
            weights: weights.id(),
            grass: grass.id(),
            table: table.id(),
            wear: wear.id(),
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
            wear,
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
        let (Some(weights), Some(grass), Some(table), Some(wear)) = (
            images.get(s.weights),
            images.get(s.grass),
            buffers.get(s.table),
            images.get(s.wear),
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
            let first = t.slot * WEIGHT_LAYERS as u32;
            let layers = t
                .weights
                .iter()
                .enumerate()
                .map(|(i, data)| (weights, t.weight_side, first + i as u32, data))
                .chain([
                    (grass, t.cover_side, t.slot, &t.cover),
                    (wear, WEAR_SIDE, t.slot, &t.wear),
                ]);
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
        }
        queue.write_buffer(&table.buffer, 0, &update.table);
        s.uploads += update.tiles.len() as u64;
        s.ready = true;
        true
    });
}
