//! Fixed-capacity material cache. Texture writes and the lookup table publish together.
use super::*;
use bevy::{
    asset::AssetId,
    core_pipeline::schedule::camera_driver,
    render::{
        RenderApp,
        render_asset::RenderAssets,
        render_resource::{
            Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
            TextureViewDescriptor, TextureViewDimension,
        },
        renderer::{RenderGraph, RenderQueue},
        storage::GpuShaderBuffer,
        texture::GpuImage,
    },
};
use std::sync::{Arc, Mutex, Weak};

pub const DETAIL_SLOTS: usize = 128;
pub const TABLE_SIZE: usize = 512;
pub const DETAIL_BYTES: u64 =
    (DETAIL_SLOTS * (72 * 72 + 36 * 36 + 18 * 18) * 8 + TABLE_SIZE * 32) as u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct DetailEntry {
    /// Canonical node X, Z, level, physical layer + 1 (zero means vacant).
    pub key: IVec4,
    /// Temporal availability, remaining fields reserved.
    pub state: Vec4,
}
pub fn hash(key: world::TerrainNodeKey) -> usize {
    let mut h = (key.x as u32).wrapping_mul(0x9e3779b9)
        ^ (key.z as u32).wrapping_mul(0x85ebca6b)
        ^ (key.level as u32).wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15;
    h as usize & (TABLE_SIZE - 1)
}
pub fn table(
    entries: impl IntoIterator<Item = (TerrainMaterialKey, u32, f32)>,
) -> Vec<DetailEntry> {
    let mut table = vec![DetailEntry::default(); TABLE_SIZE];
    for (count, (key, slot, fade)) in entries.into_iter().enumerate() {
        assert!(slot < DETAIL_SLOTS as u32 && count < DETAIL_SLOTS);
        let mut i = hash(key.0);
        while table[i].key.w != 0 {
            i = (i + 1) & (TABLE_SIZE - 1);
        }
        table[i] = DetailEntry {
            key: IVec4::new(key.0.x, key.0.z, key.0.level as i32, slot as i32 + 1),
            state: Vec4::new(fade.clamp(0., 1.), 0., 0., 0.),
        };
    }
    table
}

struct Update {
    table: Vec<u8>,
    tiles: Vec<(u32, TerrainComposite)>,
}
struct Shared {
    color: AssetId<Image>,
    response: AssetId<Image>,
    table: AssetId<ShaderBuffer>,
    pending: Option<Update>,
    ready: bool,
    uploads: u64,
}

#[derive(Resource, Clone, Default)]
pub struct CompositeUploadHub(Arc<Mutex<Vec<Weak<Mutex<Shared>>>>>);

pub struct DetailAtlas {
    pub(super) color: Handle<Image>,
    pub(super) response: Handle<Image>,
    pub(super) table: Handle<ShaderBuffer>,
    shared: Arc<Mutex<Shared>>,
}
impl DetailAtlas {
    pub fn new(
        images: &mut Assets<Image>,
        buffers: &mut Assets<ShaderBuffer>,
        hub: &CompositeUploadHub,
    ) -> Self {
        let image = |srgb| {
            let mut image = composite_image(Vec::new(), srgb);
            image.data = None;
            image.texture_descriptor.size.depth_or_array_layers = DETAIL_SLOTS as u32;
            image.texture_view_descriptor = Some(TextureViewDescriptor {
                dimension: Some(TextureViewDimension::D2Array),
                ..default()
            });
            image
        };
        let color = images.add(image(true));
        let response = images.add(image(false));
        let table = buffers.add(ShaderBuffer::with_size(
            TABLE_SIZE * 32,
            RenderAssetUsages::RENDER_WORLD,
        ));
        let shared = Arc::new(Mutex::new(Shared {
            color: color.id(),
            response: response.id(),
            table: table.id(),
            pending: None,
            ready: false,
            uploads: 0,
        }));
        hub.0.lock().unwrap().push(Arc::downgrade(&shared));
        let atlas = Self {
            color,
            response,
            table,
            shared,
        };
        atlas.submit(vec![DetailEntry::default(); TABLE_SIZE], vec![]);
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
    /// Call only after idle. New slot bytes and the table dropping their previous
    /// occupants are one render transaction, so reuse never exposes another tile.
    pub fn submit(&self, entries: Vec<DetailEntry>, tiles: Vec<(u32, TerrainComposite)>) {
        assert_eq!(entries.len(), TABLE_SIZE);
        assert!(tiles.len() <= 4);
        assert!(
            tiles
                .iter()
                .all(|(i, t)| (*i as usize) < DETAIL_SLOTS && t.validate().is_ok())
        );
        let mut s = self.shared.lock().unwrap();
        assert!(s.pending.is_none());
        s.pending = Some(Update {
            table: ShaderBuffer::from(entries).data.unwrap(),
            tiles,
        });
    }
}

pub(crate) fn install(app: &mut App) {
    let hub = CompositeUploadHub::default();
    app.insert_resource(hub.clone()).add_systems(
        PostUpdate,
        fallback_buffer
            .after(crate::TerrainMaterialPreparation)
            .after(crate::near::NearUpdate),
    );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .insert_resource(hub)
            .add_systems(RenderGraph, upload.before(camera_driver));
    }
}
fn fallback_buffer(
    mut fallback: Local<Option<Handle<ShaderBuffer>>>,
    mut near_fallback: Local<Option<Handle<ShaderBuffer>>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut materials: ResMut<Assets<TerrainCompositeMaterial>>,
) {
    let fallback = fallback.get_or_insert_with(|| {
        buffers.add(ShaderBuffer::with_size(32, RenderAssetUsages::RENDER_WORLD))
    });
    let near_fallback = near_fallback.get_or_insert_with(|| {
        buffers.add(ShaderBuffer::with_size(
            crate::near::gpu::NearEntry::min_size().get() as usize,
            RenderAssetUsages::RENDER_WORLD,
        ))
    });
    let missing: Vec<_> = materials
        .iter()
        .filter(|(_, m)| m.detail_table == Handle::default() || m.near_table == Handle::default())
        .map(|(id, _)| id)
        .collect();
    for id in missing {
        let mut m = materials.get_mut(id).unwrap();
        if m.detail_table == Handle::default() {
            m.detail_table = fallback.clone();
        }
        if m.near_table == Handle::default() {
            m.near_table = near_fallback.clone();
        }
    }
}
fn upload(
    hub: Res<CompositeUploadHub>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
) {
    hub.0.lock().unwrap().retain(|weak| {
        let Some(shared) = weak.upgrade() else {
            return false;
        };
        let mut s = shared.lock().unwrap();
        let (Some(color), Some(response), Some(table)) = (
            images.get(s.color),
            images.get(s.response),
            buffers.get(s.table),
        ) else {
            return true;
        };
        let Some(update) = s.pending.take() else {
            return true;
        };
        for (slot, tile) in &update.tiles {
            for (level, mip) in tile.mips.iter().enumerate() {
                let size = TerrainComposite::mip_size(level) as u32;
                for (image, data) in [(color, &mip.color), (response, &mip.response)] {
                    queue.write_texture(
                        TexelCopyTextureInfo {
                            texture: &image.texture,
                            mip_level: level as u32,
                            origin: Origin3d {
                                x: 0,
                                y: 0,
                                z: *slot,
                            },
                            aspect: TextureAspect::All,
                        },
                        data,
                        TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(size * 4),
                            rows_per_image: Some(size),
                        },
                        Extent3d {
                            width: size,
                            height: size,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
        }
        queue.write_buffer(&table.buffer, 0, &update.table);
        s.uploads += update.tiles.len() as u64;
        s.ready = true;
        true
    });
}
