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
mod body;
mod outfit;
mod solver;

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
use solver::{Capsule, PieceSim, StepParams};
use std::{collections::HashMap, sync::Arc, time::Instant};

pub struct ClothPlugin;

impl Plugin for ClothPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClothSettings>()
            .init_resource::<ClothStats>()
            .init_resource::<ClothCache>()
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
    /// Wind speed, m/s, at the vegetation wind's unit force.
    pub wind_speed: f32,
    pub gravity: f32,
    /// Farther from the camera, pieces hang where the animation puts them.
    pub simulate_within: f32,
    pub show_colliders: bool,
    pub show_particles: bool,
}

impl Default for ClothSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            substep_hz: 240.0,
            inertia: None,
            wind_speed: 5.0,
            gravity: 9.81,
            simulate_within: 40.0,
            show_colliders: false,
            show_particles: false,
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
}

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
}

struct RigPiece {
    sim: PieceSim,
    joints: Vec<Entity>,
    poses: Vec<(Quat, Vec3A)>,
    near: Vec<(Capsule, Capsule)>,
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
) {
    for (actor, skeleton, rig) in &actors {
        if skeleton
            .joints
            .values()
            .all(|(joint, _)| posed.contains(*joint))
        {
            continue;
        }
        info!("Cloth: actor {actor} has a new model; refitting its outfit");
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
    mut actors: Query<(
        Entity,
        &TestOutfit,
        &CharacterSkeleton,
        Option<&mut ClothRig>,
    )>,
) {
    for (actor, outfit, skeleton, rig) in &mut actors {
        if rig.as_ref().is_some_and(|rig| rig.choice == outfit.0) {
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
                let plan = outfit::plan(outfit.0, &binds, &body);
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
                .or_insert_with(|| materials.add(material(look)));
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
            pieces.push(RigPiece {
                sim: PieceSim::new(&plan.def),
                joints,
                poses: Vec::new(),
                near: Vec::new(),
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
    if let Err(error) = mesh.generate_skinned_mesh_bounds() {
        warn!("Cloth: no skinned bounds for a generated mesh: {error}");
    }
    mesh
}

fn material(look: Look) -> StandardMaterial {
    let (rgb, roughness, metallic, sheet) = match look {
        Look::Wool => ([0.30, 0.055, 0.045], 0.95, 0.0, true),
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
    let wind_direction = wind
        .as_ref()
        .map_or(Vec2::ZERO, |w| w.direction.normalize_or_zero());
    let offset = wind_origin.map_or([0.0; 2], |o| o.world_xz);
    let settings = settings.clone();
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
        let strength = wind.as_ref().map_or(0.0, |w| {
            w.sample_force(Vec2::new(
                (f64::from(root.x) + offset[0]) as f32,
                (f64::from(root.z) + offset[1]) as f32,
            ))
        });
        let params = StepParams {
            dt,
            substep_hz: settings.substep_hz,
            gravity: Vec3A::new(0.0, -settings.gravity, 0.0),
            wind: Vec3A::new(wind_direction.x, 0.0, wind_direction.y)
                * (strength * settings.wind_speed),
            inertia: settings.inertia,
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
            };
            let before = if jumped { now } else { rig.capsules[k].1 };
            rig.capsules[k] = (before, now);
        }
        let mut provided = plan.capsules.len();
        for (piece, state) in plan.pieces.iter().zip(&mut rig.pieces) {
            if simulate {
                state.near.clear();
                state.near.extend(
                    rig.capsules
                        .iter()
                        .zip(&rig.groups)
                        .filter(|(_, g)| *g & piece.collides != 0)
                        .map(|(c, _)| *c),
                );
                state
                    .sim
                    .step(&piece.def, &rig.deforms, &state.near, &params);
            } else {
                state.sim.reset(&piece.def, &rig.deforms);
            }
            state.sim.joints(&piece.def, &rig.deforms, &mut state.poses);
            let x = state.sim.positions();
            for p in &piece.provides {
                let now = Capsule::new(x[p.a as usize], x[p.b as usize], p.radius);
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
    for (rig, _) in &rigs {
        characters += 1;
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
    };
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
