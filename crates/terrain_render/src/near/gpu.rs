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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pack {
    pub base: Handle<Image>,
    pub normal: Handle<Image>,
    pub macro_image: Handle<Image>,
    pub prepared: Option<Handle<Image>>,
    pub period: f32,
}
impl Pack {
    pub fn from_material(m: &TerrainMaterial) -> Self {
        Self {
            base: m.source_base_color_array.clone(),
            normal: m.normal_material_array.clone(),
            macro_image: m.macro_variation.clone(),
            prepared: m.prepared_albedo.then(|| m.base_color_array.clone()),
            period: m.settings.surface_layers.w,
        }
    }
    pub fn matches(&self, m: &TerrainMaterial) -> bool {
        self.base == m.source_base_color_array
            && self.normal == m.normal_material_array
            && self.macro_image == m.macro_variation
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub(crate) struct NearEntry {
    pub key: IVec4,
    pub state: Vec4,
    pub layers: Vec4,
    pub sizes: Vec4,
    pub normals: Vec4,
    pub roughness: Vec4,
    pub macro_scales: Vec4,
    pub macro_settings: Vec4,
    pub phase0: Vec4,
    pub phase1: Vec4,
    pub lattice0: Vec4,
    pub lattice1: Vec4,
    pub macro0: Vec4,
    pub macro1: Vec4,
    pub macro2: Vec4,
    pub canopy: TerrainCanopyShading,
}
pub(super) fn bytes() -> u64 {
    u64::from(WEIGHT_SIDE * WEIGHT_SIDE + CANOPY_SIDE * CANOPY_SIDE) * 2 * NEAR_SLOTS as u64
        + NearEntry::min_size().get() * NEAR_TABLE as u64
}
fn phase(v: [f64; 2]) -> Vec2 {
    Vec2::new(v[0].rem_euclid(1.) as f32, v[1].rem_euclid(1.) as f32)
}
fn transformed(p: [f64; 2]) -> [f64; 2] {
    [p[0] + p[1] * 0.5773502692, p[1] * 1.1547005384]
}
fn uv_phase(p: [f64; 2], size: f32, period: f32) -> (Vec4, Vec4) {
    let uv = p.map(|v| v / f64::from(size.max(0.001)));
    let lattice = transformed(uv);
    let a = phase(uv);
    let b = phase(lattice.map(|v| v / f64::from(period.max(1.))));
    let f = phase(lattice);
    (
        Vec4::new(a.x, a.y, b.x, b.y),
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
    pack: &Pack,
    slot: u32,
    fade: f32,
) -> NearEntry {
    let settings = m.settings;
    let p = s.key.cell.origin(s.cell_size);
    let period = if settings.macro_settings.z >= 0.5
        && (s.layers.len() == 1 || settings.macro_settings.w >= 0.5)
    {
        pack.period
    } else {
        0.
    };
    let (phase0, lattice0) = uv_phase(p, settings.tile_sizes.x, period);
    let (phase1, lattice1) = uv_phase(p, settings.tile_sizes.y, period);
    let macro_phase = |i, p: [f64; 2]| {
        phase(p.map(|v| v / f64::from(settings.macro_scales[i])))
            .extend(0.)
            .extend(0.)
    };
    NearEntry {
        key: IVec4::new(s.key.cell.x, s.key.cell.z, 0, slot as i32 + 1),
        state: Vec4::new(
            fade,
            if s.surfaces.len() == 1 {
                1.
            } else {
                s.weights[0].resolution as f32
            },
            s.surfaces.len() as f32,
            period,
        ),
        layers: Vec4::new(
            settings.surface_layers.x,
            settings.surface_layers.y,
            settings.macro_settings.z,
            settings.macro_settings.w,
        ),
        sizes: Vec4::new(
            settings.tile_sizes.x,
            settings.tile_sizes.y,
            s.cell_size,
            0.,
        ),
        normals: settings.normal_settings,
        roughness: settings.roughness_ranges,
        macro_scales: settings.macro_scales,
        macro_settings: settings.macro_settings,
        phase0,
        phase1,
        lattice0,
        lattice1,
        macro0: macro_phase(0, p),
        macro1: macro_phase(
            1,
            [p[0] * 0.819 - p[1] * 0.574, p[0] * 0.574 + p[1] * 0.819],
        ),
        macro2: macro_phase(
            2,
            [p[0] * -0.342 - p[1] * 0.940, p[0] * 0.940 + p[1] * -0.342],
        ),
        canopy: m.canopy_shading,
    }
}
pub(super) struct Tile {
    slot: u32,
    weights: Vec<u8>,
    weight_side: u32,
    canopy: Vec<u8>,
}
pub(super) fn tile(
    slot: u32,
    s: &NearSource,
    m: &TerrainMaterial,
    canopy: Option<&Image>,
    origin: CellCoord,
) -> Tile {
    let (weight_side, weights) = if s.surfaces.len() == 1 {
        (1, vec![255, 0])
    } else {
        let w = &s.weights[0];
        (
            w.resolution as u32,
            w.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1]]).collect(),
        )
    };
    let minimum = [
        i64::from(s.key.cell.x) - i64::from(origin.x),
        i64::from(s.key.cell.z) - i64::from(origin.z),
    ]
    .map(|v| v as f64 * s.cell_size as f64);
    // Resample already filtered canopy coverage at 128 interior samples plus a halo.
    let mut pixels = vec![0; (CANOPY_SIDE * CANOPY_SIDE * 2) as usize];
    if let Some(image) = canopy.filter(|i| i.texture_descriptor.format == TextureFormat::Rg8Unorm)
        && let Some(data) = &image.data
    {
        let side = image.width();
        let height = image.height();
        for y in 0..CANOPY_SIDE {
            for x in 0..CANOPY_SIDE {
                let p = Vec2::new(
                    (minimum[0] + (x as f64 - 0.5) / 128. * s.cell_size as f64) as f32,
                    (minimum[1] + (y as f64 - 0.5) / 128. * s.cell_size as f64) as f32,
                );
                let uv = (p - m.canopy_bounds.xy()) * m.canopy_bounds.zw();
                let q = uv * Vec2::new(side as f32, height as f32) - Vec2::splat(0.5);
                let low = q.floor();
                let f = q - low;
                for c in 0..2 {
                    let at = |dx: f32, dy: f32| {
                        let a = (low.x + dx).clamp(0., side as f32 - 1.) as usize;
                        let b = (low.y + dy).clamp(0., height as f32 - 1.) as usize;
                        data[(b * side as usize + a) * 2 + c] as f32
                    };
                    let a = at(0., 0.) * (1. - f.x) + at(1., 0.) * f.x;
                    let b = at(0., 1.) * (1. - f.x) + at(1., 1.) * f.x;
                    pixels[((y * CANOPY_SIDE + x) * 2) as usize + c] =
                        (a * (1. - f.y) + b * f.y).round().clamp(0., 255.) as u8;
                }
            }
        }
    }
    Tile {
        slot,
        weights,
        weight_side,
        canopy: pixels,
    }
}
struct Update {
    table: Vec<u8>,
    tiles: Vec<Tile>,
}
struct Shared {
    weights: AssetId<Image>,
    canopy: AssetId<Image>,
    table: AssetId<ShaderBuffer>,
    pack: Pack,
    pending: Option<Update>,
    ready: bool,
    uploads: u64,
}
#[derive(Resource, Default, Clone)]
pub(super) struct NearUploadHub(Arc<Mutex<Vec<Weak<Mutex<Shared>>>>>);
pub(crate) struct NearAtlas {
    pub weights: Handle<Image>,
    pub canopy: Handle<Image>,
    pub table: Handle<ShaderBuffer>,
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
        let image = |side| {
            let mut image = Image::new_uninit(
                Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: NEAR_SLOTS as u32,
                },
                TextureDimension::D2,
                TextureFormat::Rg8Unorm,
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
        let weights = images.add(image(WEIGHT_SIDE));
        let canopy = images.add(image(CANOPY_SIDE));
        let table = buffers.add(ShaderBuffer::with_size(
            (NearEntry::min_size().get() * NEAR_TABLE as u64) as usize,
            RenderAssetUsages::RENDER_WORLD,
        ));
        let shared = Arc::new(Mutex::new(Shared {
            weights: weights.id(),
            canopy: canopy.id(),
            table: table.id(),
            pack: pack.clone(),
            pending: None,
            ready: false,
            uploads: 0,
        }));
        hub.0.lock().unwrap().push(Arc::downgrade(&shared));
        let atlas = Self {
            weights,
            canopy,
            table,
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
            table: ShaderBuffer::from(table).data.unwrap(),
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
        let (Some(weights), Some(canopy), Some(table)) = (
            images.get(s.weights),
            images.get(s.canopy),
            buffers.get(s.table),
        ) else {
            return true;
        };
        if [&s.pack.base, &s.pack.normal, &s.pack.macro_image]
            .into_iter()
            .chain(s.pack.prepared.as_ref())
            .any(|h| images.get(h).is_none())
        {
            return true;
        }
        let Some(update) = s.pending.take() else {
            return true;
        };
        for t in &update.tiles {
            for (image, side, data) in [
                (weights, t.weight_side, &t.weights),
                (canopy, CANOPY_SIDE, &t.canopy),
            ] {
                queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: &image.texture,
                        mip_level: 0,
                        origin: Origin3d {
                            x: 0,
                            y: 0,
                            z: t.slot,
                        },
                        aspect: TextureAspect::All,
                    },
                    data,
                    TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(side * 2),
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
