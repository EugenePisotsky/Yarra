//! Far objects: the impostors of static objects, streamed by block of cells far beyond the
//! cells' object residency (`world_db` far-object pages). Each block draws one batch per
//! impostor, whose shader shows every instance from its mesh LOD's hand-off outward; the
//! cells draw only meshes.
use super::database::{
    DatabaseRequest, FarObjectPayloads, NotSent, RequestId, WorldDatabaseWorker,
};
use super::{ActiveWorldSpace, StreamPhase, WorldCatalog, WorldOrigin, WorldStream};
use crate::trees::impostor::{ImpostorBatch, ImpostorInstance};
use bevy::{math::DVec2, prelude::*};
use std::collections::HashMap;
use world::{CellCoord, FAR_OBJECT_BLOCK_CELLS, FarObjectsPage, WorldSpaceId};

/// Blocks with any point within this distance of the camera are drawn...
const RADIUS_METRES: f64 = 2048.0;
/// ...and kept until all of them lie this much further away.
const HYSTERESIS_METRES: f64 = 256.0;
const MAX_BLOCKS_PER_REQUEST: usize = 16;
/// Each attached block spawns a batch per impostor, whose mesh is built when it loads.
const MAX_ATTACHMENTS_PER_FRAME: usize = 4;

#[derive(Resource, Default)]
pub(super) struct FarObjects {
    generation: String,
    space: Option<WorldSpaceId>,
    /// The render origin the drawn blocks are placed for.
    origin: CellCoord,
    blocks: HashMap<CellCoord, Block>,
    /// The request awaiting its reply, and its blocks.
    requested: Option<(RequestId, Vec<CellCoord>)>,
    arrived: FarObjectPayloads,
}

enum Block {
    Requested,
    Empty,
    Drawn(Entity),
    Failed,
}

/// The root of a drawn block, placed at the block's lowest corner.
#[derive(Component)]
pub(super) struct FarObjectBlock;

impl FarObjects {
    pub(super) fn receive(&mut self, id: RequestId, result: Result<FarObjectPayloads, String>) {
        // A generation or world change clears the request, so its reply is no longer awaited.
        if self.requested.as_ref().is_none_or(|(sent, _)| *sent != id) {
            return;
        }
        let (_, requested) = self.requested.take().unwrap();
        match result {
            Ok(blocks) => self.arrived.extend(blocks),
            Err(error) => {
                warn!("far objects could not be read: {error}");
                for block in requested {
                    if let Some(state @ Block::Requested) = self.blocks.get_mut(&block) {
                        *state = Block::Failed;
                    }
                }
            }
        }
    }

    fn clear(&mut self, commands: &mut Commands) {
        for (_, block) in self.blocks.drain() {
            if let Block::Drawn(entity) = block {
                commands.entity(entity).despawn();
            }
        }
        self.requested = None;
        self.arrived.clear();
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    mut commands: Commands,
    server: Res<AssetServer>,
    worker: Option<Res<WorldDatabaseWorker>>,
    stream: Res<WorldStream>,
    catalog: Res<WorldCatalog>,
    active: Res<ActiveWorldSpace>,
    origin: Res<WorldOrigin>,
    camera: crate::ActiveWorldView,
    mut far: ResMut<FarObjects>,
    mut roots: Query<&mut Transform, With<FarObjectBlock>>,
) {
    let (Some(worker), Some(manifest), StreamPhase::Ready) =
        (worker, stream.manifest.as_ref(), &stream.phase)
    else {
        return;
    };
    if far.generation != catalog.generation_id || far.space != active.current {
        far.clear(&mut commands);
        far.generation = catalog.generation_id.clone();
        far.space = active.current;
    }
    let Some(space) = far.space.and_then(|id| manifest.world_space(id)) else {
        return;
    };
    if origin.space() != Some(space.id) {
        return;
    }
    let size = f64::from(space.cell_size) * f64::from(FAR_OBJECT_BLOCK_CELLS);
    let place = |block: CellCoord| {
        let cells =
            |b: i32, o: i32| i64::from(b) * i64::from(FAR_OBJECT_BLOCK_CELLS) - i64::from(o);
        Vec3::new(
            (cells(block.x, origin.cell().x) as f64 * f64::from(space.cell_size)) as f32,
            0.0,
            (cells(block.z, origin.cell().z) as f64 * f64::from(space.cell_size)) as f32,
        )
    };
    if far.origin != origin.cell() {
        far.origin = origin.cell();
        for (&block, state) in &far.blocks {
            if let Block::Drawn(entity) = state
                && let Ok(mut transform) = roots.get_mut(*entity)
            {
                transform.translation = place(block);
            }
        }
    }

    let arrived: Vec<_> = {
        let count = far.arrived.len().min(MAX_ATTACHMENTS_PER_FRAME);
        far.arrived.drain(..count).collect()
    };
    for (block, payload) in arrived {
        if !matches!(far.blocks.get(&block), Some(Block::Requested)) {
            continue;
        }
        let state = match payload.map(|bytes| {
            FarObjectsPage::decode(&bytes)
                .map_err(|error| error.to_string())
                .and_then(|page| page.validate().map(|()| page))
        }) {
            None => Block::Empty,
            Some(Ok(page)) => Block::Drawn(spawn_block(
                &mut commands,
                &server,
                block,
                page,
                place(block),
            )),
            Some(Err(error)) => {
                warn!("far objects of block {block:?} are invalid: {error}");
                Block::Failed
            }
        };
        far.blocks.insert(block, state);
    }

    let Some(camera) = camera.active() else {
        return;
    };
    let base = origin.cell().origin(space.cell_size);
    let eye = DVec2::new(
        f64::from(camera.transform.translation().x) + base[0],
        f64::from(camera.transform.translation().z) + base[1],
    );
    if !eye.is_finite() {
        return;
    }
    let distance = |block: CellCoord| {
        let low = DVec2::new(f64::from(block.x), f64::from(block.z)) * size;
        eye.distance(eye.clamp(low, low + size))
    };
    let mut gone = Vec::new();
    for (&block, state) in &far.blocks {
        if distance(block) > RADIUS_METRES + HYSTERESIS_METRES {
            if let Block::Drawn(entity) = state {
                commands.entity(*entity).despawn();
            }
            gone.push(block);
        }
    }
    for block in gone {
        far.blocks.remove(&block);
    }

    if far.requested.is_some() {
        return;
    }
    let block_of = |v: f64| (v / size).floor() as i32;
    let (low, high) = (eye - RADIUS_METRES, eye + RADIUS_METRES);
    let mut missing: Vec<(f64, CellCoord)> = Vec::new();
    for x in block_of(low.x)..=block_of(high.x) {
        for z in block_of(low.y)..=block_of(high.y) {
            let block = CellCoord { x, z };
            let d = distance(block);
            if d <= RADIUS_METRES && !far.blocks.contains_key(&block) {
                missing.push((d, block));
            }
        }
    }
    if missing.is_empty() {
        return;
    }
    missing.sort_by(|a, b| a.0.total_cmp(&b.0));
    let blocks: Vec<_> = missing
        .into_iter()
        .take(MAX_BLOCKS_PER_REQUEST)
        .map(|(_, block)| block)
        .collect();
    match worker.send(DatabaseRequest::ReadFarObjects {
        generation: far.generation.clone(),
        space: space.id,
        blocks: blocks.clone(),
    }) {
        Ok(id) => {
            for &block in &blocks {
                far.blocks.insert(block, Block::Requested);
            }
            far.requested = Some((id, blocks));
        }
        Err(NotSent::Full) => {}
        Err(NotSent::Stopped) => warn!("far objects: database request channel closed"),
    }
}

fn spawn_block(
    commands: &mut Commands,
    server: &AssetServer,
    block: CellCoord,
    page: FarObjectsPage,
    translation: Vec3,
) -> Entity {
    let mut by_form = vec![Vec::new(); page.forms.len()];
    for instance in &page.instances {
        let form = usize::from(instance.form);
        by_form[form].push(ImpostorInstance {
            translation: Vec3::from(instance.translation),
            yaw: instance.yaw,
            scale: instance.scale,
            switch: page.forms[form].switch * instance.scale,
        });
    }
    commands
        .spawn((
            FarObjectBlock,
            Transform::from_translation(translation),
            Visibility::default(),
            Name::new(format!("Far objects {}, {}", block.x, block.z)),
        ))
        .with_children(|children| {
            for (form, instances) in page.forms.iter().zip(by_form) {
                if !instances.is_empty() {
                    children.spawn((
                        Transform::default(),
                        Visibility::default(),
                        ImpostorBatch {
                            descriptor: server.load(form.uri.clone()),
                            instances,
                        },
                    ));
                }
            }
        })
        .id()
}
