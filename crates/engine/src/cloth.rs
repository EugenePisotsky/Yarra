//! Cloth on characters, for the cloth lab: pieces are ordinary skinned meshes whose extra
//! joints are particles the [`solver`] moves, so shadows, the depth prepass and motion vectors
//! (Bevy keeps each skin's previous joints) need nothing of their own. Rigid items ride on one
//! body bone the same way. The body is stood in for by capsules fitted to its bind pose
//! ([`body`]); carried items add their own capsules, so a cloak drapes over a sword on the back.
//!
//! An actor opts in with a [`TestOutfit`]. Once its presentation is ready its skeleton is found,
//! the outfit planned ([`outfit`], cached per body mesh and choice) and its entities spawned
//! under the actor. Each frame, after transform propagation, every rig steps in parallel and its
//! joints' `GlobalTransform`s are written before bounds and skins are read.
mod authored;
mod body;
mod outfit;
mod solver;

pub use authored::{Fabric, Garment};
pub use outfit::OutfitChoice;

use crate::{WorldViewCamera, character::CharacterPresentationReady};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::{DynamicSkinnedMeshBounds, VisibilitySystems},
    math::{Affine3A, Vec3A},
    mesh::{
        Indices, VertexAttributeValues,
        skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    },
    prelude::*,
    render::render_resource::PrimitiveTopology,
    transform::TransformSystems,
};
use body::BodyCapsule;
use outfit::{Look, MeshData, OutfitPlan};
use solver::{Capsule, Layer, PieceSim, StepParams};
use std::{collections::HashMap, sync::Arc, time::Instant};

pub struct ClothPlugin;

impl Plugin for ClothPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClothSettings>()
            .init_resource::<ClothStats>()
            .init_resource::<ClothCache>()
            .init_resource::<ClothGarments>()
            .add_systems(
                Update,
                (drop_stale_rigs, find_skeletons, build_rigs).chain(),
            )
            .add_systems(
                PostUpdate,
                (simulate_cloth, draw_cloth_debug)
                    .chain()
                    .after(TransformSystems::Propagate)
                    .before(VisibilitySystems::CalculateBounds),
            );
    }
}

/// The test pieces an actor wears; changing it rebuilds them.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestOutfit(pub OutfitChoice);

/// Garments authored offline that replace the generated ones (see [`Garment`]), and fabrics
/// for the cloak and the skirt in place of plain colours.
#[derive(Resource, Clone, Default)]
pub struct ClothGarments {
    pub cloak: Option<Arc<Garment>>,
    pub skirt: Option<Arc<Garment>>,
    pub cloak_fabric: Option<Fabric>,
    pub skirt_fabric: Option<Fabric>,
}

/// Added once an actor's outfit is built and simulating.
#[derive(Component)]
pub struct ClothWorn;

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct ClothSettings {
    pub enabled: bool,
    /// Substeps per second of simulated time.
    pub substep_hz: f32,
    /// Replaces each piece's own share of world movement it resists (see the solver).
    pub inertia: Option<f32>,
    /// Wind speed, m/s, at the vegetation wind's unit force (the wind grass and trees bend
    /// in; weather scales it).
    pub wind_speed: f32,
    /// Replaces that wind: the compass bearing it blows towards (0 is +Z) and its speed, m/s.
    pub wind_override: Option<(f32, f32)>,
    /// How much the wind varies across a piece, 0–1.
    pub flutter: f32,
    /// Scale every piece's damping and its lift (drag along its surface normal).
    pub damping: f32,
    pub lift: f32,
    /// Share of a collider's push applied each substep.
    pub contact: f32,
    pub self_collide: bool,
    pub gravity: f32,
    /// Farther from the camera, pieces hang where the animation puts them.
    pub simulate_within: f32,
    pub show_colliders: bool,
    pub show_particles: bool,
    /// Count draped garments' drawn vertices that go through things ([`PieceShake::pierced`]):
    /// a lab measure, about a millisecond a garment.
    pub measure_pierce: bool,
}

impl Default for ClothSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            substep_hz: 240.0,
            inertia: None,
            wind_speed: 7.0,
            wind_override: None,
            flutter: 0.45,
            damping: 1.0,
            lift: 1.0,
            contact: 0.6,
            self_collide: true,
            gravity: 9.81,
            simulate_within: 40.0,
            show_colliders: false,
            show_particles: false,
            measure_pierce: false,
        }
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub struct ClothStats {
    pub characters: usize,
    pub simulated: usize,
    pub particles: usize,
    pub colliders: usize,
    /// The whole cloth pass last frame, wall clock.
    pub frame_us: f32,
    /// Smoothed over about a second.
    pub mean_frame_us: f32,
    /// Summed over characters (all cores), last frame.
    pub work_us: f32,
    pub worst_character_us: f32,
    /// The wind at the last character simulated, m/s, render space.
    pub wind: Vec3,
    /// Mean frame-to-frame jitter of simulated pieces, mm ([`solver::PieceSim::shake`]), and
    /// per kind of piece beside the jitter of what the animation gives it.
    pub shake_mm: f32,
    pub shake_by_piece: Vec<PieceShake>,
}

/// One kind of piece's jitter, mm, averaged over characters.
#[derive(Clone, Copy, Debug)]
pub struct PieceShake {
    pub name: &'static str,
    pub cloth: f32,
    pub animation: f32,
    /// Drawn vertices of a draped garment more than 1 cm deeper inside a collider or the
    /// garment under it than they rest, when measured ([`ClothSettings::measure_pierce`]).
    pub pierced: f32,
    /// The same by what they went into: [`PIERCED_BY`].
    pub pierced_by: [f32; PIERCED_BY.len()],
    /// A sheet's folds over ([`solver::PieceSim::folds`]), when measured.
    pub folds: f32,
}

/// What [`PieceShake::pierced_by`] counts, in order: collider groups, then inner layers.
pub const PIERCED_BY: [&str; 9] = [
    "torso",
    "arms",
    "legs",
    "head",
    "back items",
    "hip items",
    "layers",
    "pressers",
    "garment under",
];

/// An actor's bones by name: each joint entity and its inverse bind pose.
#[derive(Component)]
struct CharacterSkeleton {
    joints: HashMap<String, (Entity, Affine3A)>,
    /// Joint names in the body skin's order, which its vertices' joint indices follow.
    order: Vec<String>,
    mesh: Handle<Mesh>,
}

/// Joints and meshes the cloth module spawned.
#[derive(Component)]
struct ClothPart;

#[derive(Component)]
struct ClothJoint;

#[derive(Resource, Default)]
struct ClothCache {
    bodies: HashMap<AssetId<Mesh>, Arc<Vec<BodyCapsule>>>,
    outfits: HashMap<(OutfitChoice, AssetId<Mesh>), Arc<BuiltOutfit>>,
    looks: HashMap<Look, Handle<StandardMaterial>>,
}

/// An outfit plan with its meshes and inverse bind poses uploaded, shared by every actor with
/// the same body mesh and choice.
struct BuiltOutfit {
    plan: OutfitPlan,
    pieces: Vec<BuiltMeshes>,
    items: Vec<BuiltMeshes>,
}

struct BuiltMeshes {
    meshes: Vec<(Handle<Mesh>, Look)>,
    bindposes: Handle<SkinnedMeshInverseBindposes>,
}

#[derive(Component)]
struct ClothRig {
    choice: OutfitChoice,
    outfit: Arc<BuiltOutfit>,
    bones: Vec<Entity>,
    inverse_binds: Vec<Affine3A>,
    deforms: Vec<Affine3A>,
    /// Plan capsules then provided ones, each at the start and end of the frame.
    capsules: Vec<(Capsule, Capsule)>,
    groups: Vec<u32>,
    pieces: Vec<RigPiece>,
    spawned: Vec<Entity>,
    last_root: Option<Vec3>,
    simulated: bool,
    time_us: f32,
    /// Sum of its simulated pieces' shake, metres, and how many.
    shake: (f32, usize),
    /// Each piece's shake this frame and its targets', metres.
    piece_shake: Vec<(f32, f32, [f32; PIERCED_BY.len()], f32)>,
}

struct RigPiece {
    sim: PieceSim,
    joints: Vec<Entity>,
    poses: Vec<(Quat, Vec3A)>,
    near: Vec<(Capsule, Capsule)>,
    /// What the pieces over it see, when it is an inner layer.
    layer: Option<Layer>,
}

type WaitingActors<'w, 's> = Query<
    'w,
    's,
    Entity,
    (
        With<TestOutfit>,
        With<CharacterPresentationReady>,
        Without<CharacterSkeleton>,
    ),
>;

/// A presentation can replace an actor's model (a new profile, a respawned scene); its old
/// bones are then gone. Forget the skeleton and its outfit so both are made again for the new one.
fn drop_stale_rigs(
    mut commands: Commands,
    actors: Query<(Entity, &CharacterSkeleton, Option<&ClothRig>)>,
    posed: Query<(), With<GlobalTransform>>,
    entities: &bevy::ecs::entity::Entities,
) {
    for (actor, skeleton, rig) in &actors {
        if skeleton
            .joints
            .values()
            .all(|(joint, _)| posed.contains(*joint))
        {
            continue;
        }
        // Seen rarely at startup for every actor at once, cause unknown: say which bones went
        // and whether they were despawned or only lost their pose.
        let missing: Vec<_> = skeleton
            .joints
            .iter()
            .filter(|(_, (joint, _))| !posed.contains(*joint))
            .map(|(name, (joint, _))| {
                let gone = if entities.contains(*joint) {
                    "unposed"
                } else {
                    "despawned"
                };
                format!("{name} {gone}")
            })
            .take(3)
            .collect();
        info!("Cloth: actor {actor} lost bones ({missing:?}); refitting its outfit");
        if let Some(rig) = rig {
            for &entity in &rig.spawned {
                commands.entity(entity).despawn();
            }
        }
        commands
            .entity(actor)
            .remove::<(CharacterSkeleton, ClothRig, ClothWorn)>();
    }
}

fn find_skeletons(
    mut commands: Commands,
    actors: WaitingActors,
    children: Query<&Children>,
    skins: Query<(&SkinnedMesh, &Mesh3d), Without<ClothPart>>,
    names: Query<&Name>,
    bindposes: Res<Assets<SkinnedMeshInverseBindposes>>,
) {
    for actor in &actors {
        let Some((skin, mesh)) = children
            .iter_descendants(actor)
            .find_map(|e| skins.get(e).ok())
        else {
            continue;
        };
        let Some(inverse) = bindposes.get(&skin.inverse_bindposes) else {
            continue;
        };
        let order: Vec<String> = skin
            .joints
            .iter()
            .map(|&joint| {
                names
                    .get(joint)
                    .map_or(String::new(), |n| n.as_str().to_owned())
            })
            .collect();
        let joints = order
            .iter()
            .zip(skin.joints.iter().zip(inverse.iter()))
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, (&joint, ibm))| (name.clone(), (joint, Affine3A::from_mat4(*ibm))))
            .collect();
        commands.entity(actor).insert(CharacterSkeleton {
            joints,
            order,
            mesh: mesh.0.clone(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn build_rigs(
    mut commands: Commands,
    mut cache: ResMut<ClothCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    server: Res<AssetServer>,
    garments: Res<ClothGarments>,
    mut actors: Query<(
        Entity,
        &TestOutfit,
        &CharacterSkeleton,
        Option<&mut ClothRig>,
    )>,
) {
    // Other garments: every outfit is planned again.
    if garments.is_changed() && !garments.is_added() {
        cache.outfits.clear();
        cache.looks.clear();
    }
    for (actor, outfit, skeleton, rig) in &mut actors {
        if rig.as_ref().is_some_and(|rig| rig.choice == outfit.0) && !garments.is_changed() {
            continue;
        }
        let mesh_id = skeleton.mesh.id();
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.bodies.entry(mesh_id) {
            let Some(body) = meshes.get(&skeleton.mesh) else {
                continue;
            };
            let capsules = fit_body(body, skeleton);
            info!(
                "Cloth: fitted {} body colliders: {}",
                capsules.len(),
                capsules
                    .iter()
                    .map(|c| format!("{} r={:.3}", c.name, c.radius))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            entry.insert(Arc::new(capsules));
        }
        let key = (outfit.0, mesh_id);
        let built = match cache.outfits.get(&key) {
            Some(built) => built.clone(),
            None => {
                let body = cache.bodies[&mesh_id].clone();
                let binds = skeleton
                    .joints
                    .iter()
                    .map(|(name, (_, ibm))| (name.clone(), ibm.inverse()))
                    .collect();
                let plan = outfit::plan(
                    outfit.0,
                    &binds,
                    &body,
                    garments.cloak.as_deref(),
                    garments.skirt.as_deref(),
                );
                let built = Arc::new(upload(plan, skeleton, &mut meshes, &mut bindposes));
                cache.outfits.insert(key, built.clone());
                built
            }
        };
        for look in built
            .pieces
            .iter()
            .chain(&built.items)
            .flat_map(|b| b.meshes.iter().map(|(_, look)| *look))
        {
            cache
                .looks
                .entry(look)
                .or_insert_with(|| materials.add(material(look, &garments, &server)));
        }

        if let Some(rig) = &rig {
            for &entity in &rig.spawned {
                commands.entity(entity).despawn();
            }
        }
        let mut spawned = Vec::new();
        let bones: Vec<Entity> = built
            .plan
            .bones
            .iter()
            .map(|name| skeleton.joints[name].0)
            .collect();
        let inverse_binds = built
            .plan
            .bones
            .iter()
            .map(|name| skeleton.joints[name].1)
            .collect();
        let mut pieces = Vec::new();
        for (plan, uploaded) in built.plan.pieces.iter().zip(&built.pieces) {
            let joints: Vec<Entity> = (0..plan.def.joint_count())
                .map(|_| {
                    commands
                        .spawn((Transform::default(), ClothJoint, ClothPart, ChildOf(actor)))
                        .id()
                })
                .collect();
            spawned.extend(&joints);
            for (mesh, look) in &uploaded.meshes {
                spawned.push(spawn_skinned(
                    &mut commands,
                    actor,
                    plan.name,
                    mesh.clone(),
                    cache.looks[look].clone(),
                    uploaded.bindposes.clone(),
                    joints.clone(),
                ));
            }
            let layer = plan.layer.and_then(|at| match plan.def.frames {
                solver::Frames::Grid { columns, rows, .. } => {
                    let angles: Vec<f32> = plan.def.particles[..columns]
                        .iter()
                        .map(|p| (p.rest.x - at.origin.x).atan2(p.rest.z - at.origin.z))
                        .collect();
                    Some(Layer::new(rows, &angles))
                }
                _ => None,
            });
            pieces.push(RigPiece {
                sim: PieceSim::new(&plan.def),
                joints,
                poses: Vec::new(),
                near: Vec::new(),
                layer,
            });
        }
        for (plan, uploaded) in built.plan.items.iter().zip(&built.items) {
            for (mesh, look) in &uploaded.meshes {
                spawned.push(spawn_skinned(
                    &mut commands,
                    actor,
                    plan.name,
                    mesh.clone(),
                    cache.looks[look].clone(),
                    uploaded.bindposes.clone(),
                    vec![bones[plan.bone as usize]],
                ));
            }
        }
        let mut groups: Vec<u32> = built.plan.capsules.iter().map(|c| c.group).collect();
        groups.extend(
            built
                .plan
                .pieces
                .iter()
                .flat_map(|p| p.provides.iter().map(|p| p.group)),
        );
        let capsule_count = groups.len();
        let bone_count = bones.len();
        commands.entity(actor).insert((
            ClothRig {
                choice: outfit.0,
                outfit: built.clone(),
                bones,
                inverse_binds,
                deforms: vec![Affine3A::IDENTITY; bone_count],
                capsules: vec![
                    (
                        Capsule::new(Vec3A::ZERO, Vec3A::ZERO, 0.0),
                        Capsule::new(Vec3A::ZERO, Vec3A::ZERO, 0.0)
                    );
                    capsule_count
                ],
                groups,
                pieces,
                spawned,
                last_root: None,
                simulated: false,
                time_us: 0.0,
                shake: (0.0, 0),
                piece_shake: Vec::new(),
            },
            ClothWorn,
        ));
    }
}

fn spawn_skinned(
    commands: &mut Commands,
    actor: Entity,
    name: &'static str,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    joints: Vec<Entity>,
) -> Entity {
    commands
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            SkinnedMesh {
                inverse_bindposes,
                joints,
            },
            DynamicSkinnedMeshBounds,
            Transform::default(),
            Visibility::Inherited,
            ClothPart,
            ChildOf(actor),
            Name::new(format!("Cloth lab {name}")),
        ))
        .id()
}

/// The body's colliders from its mesh in bind pose, or from bone positions alone when the mesh
/// keeps no vertex data.
fn fit_body(mesh: &Mesh, skeleton: &CharacterSkeleton) -> Vec<BodyCapsule> {
    let joints: Vec<(String, Affine3A)> = skeleton
        .order
        .iter()
        .map(|name| {
            let bind = skeleton
                .joints
                .get(name)
                .map_or(Affine3A::IDENTITY, |(_, ibm)| ibm.inverse());
            (name.clone(), bind)
        })
        .collect();
    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(positions)) => positions.as_slice(),
        _ => &[],
    };
    let dominant: Vec<u16> = match (
        mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX),
        mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT),
    ) {
        (
            Some(VertexAttributeValues::Uint16x4(indices)),
            Some(VertexAttributeValues::Float32x4(weights)),
        ) => indices
            .iter()
            .zip(weights)
            .map(|(j, w)| {
                let strongest = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap_or(0);
                j[strongest]
            })
            .collect(),
        _ => Vec::new(),
    };
    let positions = if dominant.len() == positions.len() {
        positions
    } else {
        warn!("Cloth: the body mesh keeps no skinning data; using default collider radii");
        &[]
    };
    body::fit(&body::BindBody {
        positions,
        dominant: &dominant,
        joints: &joints,
    })
}

fn upload(
    plan: OutfitPlan,
    skeleton: &CharacterSkeleton,
    meshes: &mut Assets<Mesh>,
    bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
) -> BuiltOutfit {
    let pieces = plan
        .pieces
        .iter()
        .map(|piece| BuiltMeshes {
            meshes: piece
                .parts
                .iter()
                .map(|(look, data)| (meshes.add(to_mesh(data)), *look))
                .collect(),
            bindposes: bindposes.add(SkinnedMeshInverseBindposes::from(
                piece
                    .def
                    .joint_rests()
                    .into_iter()
                    .map(|rest| Mat4::from_translation(-Vec3::from(rest)))
                    .collect::<Vec<_>>(),
            )),
        })
        .collect();
    let items = plan
        .items
        .iter()
        .map(|item| {
            let ibm = skeleton.joints[&plan.bones[item.bone as usize]].1;
            BuiltMeshes {
                meshes: item
                    .parts
                    .iter()
                    .map(|(look, data)| (meshes.add(to_mesh(data)), *look))
                    .collect(),
                bindposes: bindposes.add(SkinnedMeshInverseBindposes::from(vec![Mat4::from(ibm)])),
            }
        })
        .collect();
    BuiltOutfit {
        plan,
        pieces,
        items,
    }
}

fn to_mesh(data: &MeshData) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, data.positions.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs.clone())
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(data.joints.clone()),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, data.weights.clone())
    .with_inserted_indices(Indices::U32(data.indices.clone()));
    // For fabrics' normal maps.
    if let Err(error) = mesh.generate_tangents() {
        warn!("Cloth: no tangents for a generated mesh: {error}");
    }
    if let Err(error) = mesh.generate_skinned_mesh_bounds() {
        warn!("Cloth: no skinned bounds for a generated mesh: {error}");
    }
    mesh
}

fn material(look: Look, garments: &ClothGarments, server: &AssetServer) -> StandardMaterial {
    // The fabric's colour as it is for the cloak; the skirt's tinted towards undyed linen.
    let fabric = match look {
        Look::Wool => garments.cloak_fabric.as_ref().map(|f| (f, [1.0, 1.0, 1.0])),
        Look::Linen => garments
            .skirt_fabric
            .as_ref()
            .map(|f| (f, [0.85, 0.74, 0.58])),
        _ => None,
    };
    if let Some((fabric, tint)) = fabric {
        return StandardMaterial {
            base_color: Color::srgb(tint[0], tint[1], tint[2]),
            base_color_texture: Some(crate::trees::load_tiling(
                server,
                &fabric.map("color"),
                true,
            )),
            normal_map_texture: Some(crate::trees::load_tiling(
                server,
                &fabric.map("normal"),
                false,
            )),
            metallic_roughness_texture: Some(crate::trees::load_tiling(
                server,
                &fabric.map("roughness"),
                false,
            )),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(1.0 / fabric.tile)),
            double_sided: true,
            cull_mode: None,
            ..default()
        };
    }
    let (rgb, roughness, metallic, sheet) = match look {
        Look::Wool => ([0.42, 0.10, 0.07], 0.95, 0.0, true),
        Look::Linen => ([0.36, 0.31, 0.22], 0.9, 0.0, true),
        Look::Hair => ([0.11, 0.065, 0.035], 0.5, 0.0, false),
        Look::Steel => ([0.62, 0.63, 0.65], 0.35, 0.9, false),
        Look::DarkSteel => ([0.3, 0.3, 0.32], 0.45, 0.9, false),
        Look::Leather => ([0.16, 0.09, 0.05], 0.75, 0.0, false),
        Look::Wood => ([0.36, 0.21, 0.1], 0.6, 0.0, false),
        Look::Bowstring => ([0.75, 0.72, 0.62], 0.8, 0.0, false),
    };
    StandardMaterial {
        base_color: Color::srgb(rgb[0], rgb[1], rgb[2]),
        perceptual_roughness: roughness,
        metallic,
        double_sided: sheet,
        cull_mode: if sheet {
            None
        } else {
            Some(bevy::render::render_resource::Face::Back)
        },
        ..default()
    }
}

#[allow(clippy::too_many_arguments)]
fn simulate_cloth(
    time: Res<Time>,
    settings: Res<ClothSettings>,
    wind: Option<Res<vegetation_render::VegetationWind>>,
    wind_origin: Option<Res<vegetation_render::VegetationRenderOrigin>>,
    mut stats: ResMut<ClothStats>,
    cameras: Query<&GlobalTransform, (With<WorldViewCamera>, Without<ClothJoint>)>,
    poses: Query<&GlobalTransform, Without<ClothJoint>>,
    mut rigs: Query<(&mut ClothRig, &GlobalTransform), Without<ClothJoint>>,
    mut joints: Query<&mut GlobalTransform, With<ClothJoint>>,
) {
    let started = Instant::now();
    let camera = cameras.iter().next().map(|c| c.translation());
    let dt = time.delta_secs();
    let seconds = time.elapsed_secs_wrapped();
    let wind_direction = wind
        .as_ref()
        .map_or(Vec2::ZERO, |w| w.direction.normalize_or_zero());
    let offset = wind_origin.map_or([0.0; 2], |o| o.world_xz);
    let settings = settings.clone();
    let last_wind = std::sync::Mutex::new(Vec3A::ZERO);
    rigs.par_iter_mut().for_each(|(mut rig, root)| {
        let begun = Instant::now();
        let rig = &mut *rig;
        let root = root.translation();
        for (k, bone) in rig.bones.iter().enumerate() {
            if let Ok(pose) = poses.get(*bone) {
                rig.deforms[k] = pose.affine() * rig.inverse_binds[k];
            }
        }
        let far = camera.is_some_and(|c| c.distance(root) > settings.simulate_within);
        let jumped = rig
            .last_root
            .is_none_or(|last| last.distance(root) > (8.0 * dt).max(1.0));
        rig.last_root = Some(root);
        let simulate = settings.enabled && !far && !jumped;
        let blowing = match settings.wind_override {
            Some((bearing, speed)) => {
                let (s, c) = bearing.to_radians().sin_cos();
                Vec3A::new(s, 0.0, c) * speed
            }
            None => {
                let strength = wind.as_ref().map_or(0.0, |w| {
                    w.sample_force(Vec2::new(
                        (f64::from(root.x) + offset[0]) as f32,
                        (f64::from(root.z) + offset[1]) as f32,
                    ))
                });
                Vec3A::new(wind_direction.x, 0.0, wind_direction.y)
                    * (strength * settings.wind_speed)
            }
        };
        if simulate && let Ok(mut last) = last_wind.lock() {
            *last = blowing;
        }
        let params = StepParams {
            dt,
            substep_hz: settings.substep_hz,
            gravity: Vec3A::new(0.0, -settings.gravity, 0.0),
            wind: blowing,
            flutter: settings.flutter,
            time: seconds,
            inertia: settings.inertia,
            damping: settings.damping,
            lift: settings.lift,
            contact: settings.contact,
            self_collide: settings.self_collide,
        };
        let plan = &rig.outfit.plan;
        for (k, c) in plan.capsules.iter().enumerate() {
            let deform = rig.deforms[c.bone as usize];
            let now = Capsule {
                a: deform.transform_point3a(c.a),
                b: deform.transform_point3a(c.b),
                radius: c.radius,
                press: c.press.map_or(Vec3A::ZERO, |p| {
                    deform.matrix3.mul_vec3a(p).normalize_or_zero()
                }),
                width: c.width,
                // Set per piece, from its axis's groups.
                bulk: false,
                outward: false,
            };
            let before = if jumped { now } else { rig.capsules[k].1 };
            rig.capsules[k] = (before, now);
        }
        let mut provided = plan.capsules.len();
        rig.shake = (0.0, 0);
        rig.piece_shake.clear();
        for (k, piece) in plan.pieces.iter().enumerate() {
            let (inner, rest) = rig.pieces.split_at_mut(k);
            let state = &mut rest[0];
            if simulate {
                state.near.clear();
                let (hull, out) = piece.def.radial.map_or((0, 0), |r| (r.groups, r.outward));
                state.near.extend(
                    rig.capsules
                        .iter()
                        .zip(&rig.groups)
                        .filter(|(_, g)| *g & piece.collides != 0)
                        .map(|(&(before, after), g)| {
                            let (bulk, outward) = (g & hull != 0, g & out != 0);
                            let at = |c: Capsule| Capsule { bulk, outward, ..c };
                            (at(before), at(after))
                        }),
                );
                let layers: Vec<&Layer> = if piece.collides & body::group::LAYERS != 0 {
                    inner.iter().filter_map(|p| p.layer.as_ref()).collect()
                } else {
                    Vec::new()
                };
                state
                    .sim
                    .step(&piece.def, &rig.deforms, &state.near, &layers, &params);
                if piece.def.particles.iter().any(|p| !p.pinned) {
                    rig.shake.0 += state.sim.shake();
                    rig.shake.1 += 1;
                }
                rig.piece_shake.push((
                    state.sim.shake(),
                    state.sim.target_shake(),
                    [0.0; PIERCED_BY.len()],
                    0.0,
                ));
            } else {
                state.sim.reset(&piece.def, &rig.deforms);
            }
            state.sim.joints(&piece.def, &rig.deforms, &mut state.poses);
            if simulate
                && settings.measure_pierce
                && let Some(check) = &piece.pierce
                && let Some(entry) = rig.piece_shake.last_mut()
            {
                let layers: Vec<&Layer> = if piece.collides & body::group::LAYERS != 0 {
                    inner.iter().filter_map(|p| p.layer.as_ref()).collect()
                } else {
                    Vec::new()
                };
                let rests = piece.def.joint_rests();
                let groups: Vec<u32> = rig
                    .groups
                    .iter()
                    .copied()
                    .filter(|g| g & piece.collides != 0)
                    .collect();
                entry.2 = pierced(check, &rests, &state.poses, &state.near, &groups, &layers);
                entry.3 = state.sim.folds(&piece.def) as f32;
            }
            if let (Some(layer), Some(at)) = (state.layer.as_mut(), piece.layer) {
                let deform = rig.deforms[at.bone as usize];
                let (_, turn, _) = deform.to_scale_rotation_translation();
                layer.update(
                    state.sim.positions(),
                    deform.transform_point3a(at.origin),
                    bevy::math::Mat3A::from_quat(turn),
                );
            }
            let x = state.sim.positions();
            let turn = rig.deforms[piece.def.anchor as usize].matrix3;
            for p in &piece.provides {
                let now = Capsule {
                    press: p
                        .press
                        .map_or(Vec3A::ZERO, |d| turn.mul_vec3a(d).normalize_or_zero()),
                    width: p.width,
                    ..Capsule::new(x[p.a as usize], x[p.b as usize], p.radius)
                };
                let before = if jumped {
                    now
                } else {
                    rig.capsules[provided].1
                };
                rig.capsules[provided] = (before, now);
                provided += 1;
            }
        }
        rig.simulated = simulate;
        rig.time_us = begun.elapsed().as_secs_f32() * 1e6;
    });

    let (mut characters, mut simulated, mut particles, mut work, mut worst) = (0, 0, 0, 0.0, 0f32);
    let mut shake = (0.0, 0);
    let mut by_piece: Vec<(PieceShake, usize)> = Vec::new();
    for (rig, _) in &rigs {
        characters += 1;
        shake.0 += rig.shake.0;
        shake.1 += rig.shake.1;
        for (plan, &(cloth, animation, pierced_by, folds)) in
            rig.outfit.plan.pieces.iter().zip(&rig.piece_shake)
        {
            let pierced = pierced_by.iter().sum::<f32>();
            match by_piece.iter_mut().find(|(p, _)| p.name == plan.name) {
                Some((entry, n)) => {
                    entry.cloth += cloth;
                    entry.animation += animation;
                    entry.pierced += pierced;
                    entry.folds += folds;
                    for (sum, by) in entry.pierced_by.iter_mut().zip(pierced_by) {
                        *sum += by;
                    }
                    *n += 1;
                }
                None => by_piece.push((
                    PieceShake {
                        name: plan.name,
                        cloth,
                        animation,
                        pierced,
                        pierced_by,
                        folds,
                    },
                    1,
                )),
            }
        }
        simulated += usize::from(rig.simulated);
        work += rig.time_us;
        worst = worst.max(rig.time_us);
        for (plan, piece) in rig.outfit.plan.pieces.iter().zip(&rig.pieces) {
            particles += plan.def.particles.len();
            for (&joint, &(rotation, position)) in piece.joints.iter().zip(&piece.poses) {
                if let Ok(mut global) = joints.get_mut(joint) {
                    *global = GlobalTransform::from(Affine3A::from_rotation_translation(
                        rotation,
                        position.into(),
                    ));
                }
            }
        }
    }
    let frame_us = started.elapsed().as_secs_f32() * 1e6;
    let colliders = rigs.iter().map(|(rig, _)| rig.capsules.len()).sum();
    *stats = ClothStats {
        characters,
        simulated,
        particles,
        colliders,
        frame_us,
        mean_frame_us: stats.mean_frame_us + (frame_us - stats.mean_frame_us) * (dt * 2.0).min(1.0),
        work_us: work,
        worst_character_us: worst,
        wind: last_wind.into_inner().map_or(Vec3::ZERO, Vec3::from),
        shake_mm: shake.0 / shake.1.max(1) as f32 * 1000.0,
        shake_by_piece: by_piece
            .into_iter()
            .map(|(p, n)| PieceShake {
                cloth: p.cloth / n as f32 * 1000.0,
                animation: p.animation / n as f32 * 1000.0,
                ..p
            })
            .collect(),
    };
}

/// A draped piece's drawn vertices that are deeper inside a collider or an inner layer than
/// they rest, by more than [`outfit::PIERCE_DEPTH`] ([`outfit::PierceCheck`]); `rests` are the
/// render joints' rest positions, `poses` their poses now.
/// Counted by what they went into first ([`PIERCED_BY`]); `groups` are `near`'s.
fn pierced(
    check: &outfit::PierceCheck,
    rests: &[Vec3A],
    poses: &[(Quat, Vec3A)],
    near: &[(Capsule, Capsule)],
    groups: &[u32],
    layers: &[&Layer],
) -> [f32; PIERCED_BY.len()] {
    let mut count = [0.0; PIERCED_BY.len()];
    for (i, &p) in check.positions.iter().enumerate() {
        let mut x = Vec3A::ZERO;
        for (&joint, &weight) in check.joints[i].iter().zip(&check.weights[i]) {
            if weight > 0.0 {
                let (turn, at) = poses[joint as usize];
                x += (turn.mul_vec3a(p - rests[joint as usize]) + at) * weight;
            }
        }
        let rested = |k: usize| {
            check.rests[i]
                .iter()
                .find(|(c, _)| *c as usize == k)
                .map_or(0.0, |&(_, room)| room.min(0.0))
        };
        let through = near.iter().enumerate().find(|(k, (_, now))| {
            now.press == Vec3A::ZERO
                && x.distance(now.closest(x)) - now.radius < rested(*k) - outfit::PIERCE_DEPTH
        });
        if let Some((k, _)) = through {
            let bit = groups.get(k).map_or(0, |g| g.trailing_zeros() as usize);
            count[bit.min(PIERCED_BY.len() - 2)] += 1.0;
            continue;
        }
        let under = check.layer_rests.get(i).copied().unwrap_or(0.0);
        if layers.iter().any(|layer| {
            layer
                .push_out(x, 0.0)
                .is_some_and(|out| out.distance(x) > under + outfit::PIERCE_DEPTH)
        }) {
            count[PIERCED_BY.len() - 1] += 1.0;
        }
    }
    count
}

fn draw_cloth_debug(settings: Res<ClothSettings>, rigs: Query<&ClothRig>, mut gizmos: Gizmos) {
    if !settings.show_colliders && !settings.show_particles {
        return;
    }
    for rig in &rigs {
        if settings.show_colliders {
            for (&(_, c), group) in rig.capsules.iter().zip(&rig.groups) {
                let axis = c.b - c.a;
                let length = axis.length();
                let rotation =
                    Quat::from_rotation_arc(Vec3::Y, Vec3::from(axis.normalize_or(Vec3A::Y)));
                let colour = if c.press != Vec3A::ZERO {
                    Color::srgb(1.0, 0.2, 0.6)
                } else if *group & body::group::LAYERS != 0 {
                    Color::srgb(0.6, 1.0, 0.3)
                } else if *group & (body::group::BACK_ITEMS | body::group::HIP_ITEMS) != 0 {
                    Color::srgb(1.0, 0.6, 0.1)
                } else {
                    Color::srgb(0.2, 0.8, 1.0)
                };
                gizmos
                    .primitive_3d(
                        &Capsule3d::new(c.radius, length),
                        Isometry3d::new(Vec3::from((c.a + c.b) * 0.5), rotation),
                        colour,
                    )
                    .resolution(8);
            }
        }
        if settings.show_particles {
            for (plan, piece) in rig.outfit.plan.pieces.iter().zip(&rig.pieces) {
                for (p, x) in plan.def.particles.iter().zip(piece.sim.positions()) {
                    let colour = if p.pinned {
                        Color::srgb(1.0, 0.2, 0.2)
                    } else {
                        Color::srgb(0.9, 0.9, 0.2)
                    };
                    gizmos
                        .sphere(
                            Isometry3d::from_translation(Vec3::from(*x)),
                            p.radius,
                            colour,
                        )
                        .resolution(6);
                }
            }
        }
    }
}
